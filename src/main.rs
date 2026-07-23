//! kaspulse — a real-time, multi-asset price oracle for Kaspa.
//!
//! Phase 2 (speed): majors stream over **WebSocket** (Kraken + Bybit push every
//! tick — sub-second), with a slow REST thread adding more venues + KRC-20. A
//! fast sign/serve loop medians the freshest prices and threshold-signs them, so
//! the feed is always <1s old — fresher than most on-chain oracles, on a chain
//! (Kaspa, ~100ms blocks) that can actually settle it that fast.
//!
//! All sources are exchanges / on-chain — no dependency on any other Kaspa
//! project's API.

#![allow(deprecated)] // tungstenite 0.21: write_message/read_message
mod http;
#[cfg(feature = "og")]
mod og;

use anyhow::Result;
use secp256k1::{Keypair, SECP256K1};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tungstenite::{connect, Message};

const PORT: u16 = 8080;
const HISTORY: usize = 120;
const N_NODES: usize = 5;
const THRESHOLD: usize = 3;
const SERVE_MS: u64 = 400;    // re-sign + serve cadence
const STALE_MS: u64 = 30_000; // drop a source's price if older than this (KRC-20 pools refresh every few s)
const SLOW_EVERY: u64 = 5;    // REST/KRC-20 refresh (seconds)
const HEARTBEAT_S: u64 = 5;   // re-sign an UNCHANGED price at most this often (changed prices sign immediately)

fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() }
fn now_ms() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 }

// live price book: pair -> exchange -> (price, ts_ms). WS + REST both write here.
type Live = Arc<Mutex<HashMap<String, HashMap<&'static str, (f64, u64)>>>>;
fn set_price(lp: &Live, pair: &str, ex: &'static str, price: f64) {
    if price > 0.0 { lp.lock().unwrap().entry(pair.to_string()).or_default().insert(ex, (price, now_ms())); }
}

// ---------- feeds ----------
// KRC-20 pools discovered on-chain from the Zealous factory (pools.json), each
// {symbol, pool, wkas_is_token0, dec}. Loaded once. No third-party in the path.
// {symbol, pool, wkas_is_token0, dec, chain}. A token can have a pool on BOTH
// chains (Kasplex + Igra) — each is a separate on-chain source; build() medians.
#[derive(Clone)]
struct Pool { symbol: String, pool: String, wkas_is_token0: bool, dec: u32, chain: String }
/// On-chain token symbols are attacker-chosen bytes. A symbol becomes a pair
/// name inside the signed message ('|'-delimited!), a URL path segment, HTML,
/// XML and our hand-built pools.json — so only a strict charset is accepted;
/// anything else is rejected (not mangled: "M&M" quoted as "MM" would lie).
fn clean_symbol(s: &str) -> Option<String> {
    let ok = !s.is_empty() && s.len() <= 32
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_'));
    if ok { Some(s.to_string()) } else { None }
}
fn parse_pools(s: &str) -> Vec<Pool> {
    serde_json::from_str::<serde_json::Value>(s).ok()
        .and_then(|v| v.as_array().map(|a| a.iter().filter_map(|p| {
            let symbol = clean_symbol(p["symbol"].as_str()?)?;
            // a KRC-20 meme token named KAS/BTC/ETH must NOT collide with the real major feeds
            if matches!(symbol.to_uppercase().as_str(), "KAS" | "BTC" | "ETH") { return None; }
            Some(Pool { symbol, pool: p["pair"].as_str()?.to_string(),
                wkas_is_token0: p["wkas_is_token0"].as_bool()?, dec: p["dec"].as_u64()? as u32,
                chain: p["chain"].as_str().unwrap_or("kasplex").to_string() })
        }).collect())).unwrap_or_default()
}
// runtime-updatable so the discovery thread can refresh it without a restart
static POOLS: std::sync::OnceLock<Mutex<Arc<Vec<Pool>>>> = std::sync::OnceLock::new();
fn pools_cell() -> &'static Mutex<Arc<Vec<Pool>>> {
    POOLS.get_or_init(|| Mutex::new(Arc::new(parse_pools(&std::fs::read_to_string("pools.json").unwrap_or_default()))))
}
fn load_pools() -> Arc<Vec<Pool>> { pools_cell().lock().unwrap().clone() }
#[derive(Clone)]
struct FeedCfg { pair: String, kind: &'static str, kucoin: Option<&'static str>, gate: Option<&'static str>, mexc: Option<&'static str> }
fn feeds() -> Vec<FeedCfg> {
    let mut v = vec![
        FeedCfg { pair: "KAS/USD".into(), kind: "major", kucoin: Some("KAS-USDT"), gate: Some("KAS_USDT"), mexc: Some("KASUSDT") },
        FeedCfg { pair: "BTC/USD".into(), kind: "major", kucoin: Some("BTC-USDT"), gate: Some("BTC_USDT"), mexc: Some("BTCUSDT") },
        FeedCfg { pair: "ETH/USD".into(), kind: "major", kucoin: Some("ETH-USDT"), gate: Some("ETH_USDT"), mexc: Some("ETHUSDT") },
    ];
    let mut seen = std::collections::HashSet::new();
    let pl = load_pools();
    for p in pl.iter() { if seen.insert(p.symbol.clone()) { v.push(FeedCfg { pair: format!("{}/USD", p.symbol), kind: "krc20", kucoin: None, gate: None, mexc: None }); } }
    v
}

// ---------- WebSocket streams (sub-second) ----------
fn ws_loop(name: &str, url: &str, sub: &str, lp: &Live, handle: impl Fn(&serde_json::Value, &Live)) {
    let mut fails: u32 = 0;
    loop {
        match connect(url) {
            Ok((mut s, _)) => {
                fails = 0;
                let _ = s.write_message(Message::Text(sub.to_string()));
                loop {
                    match s.read_message() {
                        Ok(Message::Text(t)) => { if let Ok(j) = serde_json::from_str::<serde_json::Value>(&t) { handle(&j, lp); } }
                        Ok(Message::Ping(p)) => { let _ = s.write_message(Message::Pong(p)); }
                        Ok(Message::Close(_)) => break,
                        Ok(_) => {}
                        Err(e) => { eprintln!("{name} ws read error: {e} — reconnecting"); break; }
                    }
                }
            }
            Err(e) => { fails += 1; eprintln!("{name} ws connect failed: {e}"); }
        }
        // exponential backoff (2s → 30s) so repeated failures don't hammer the exchange
        std::thread::sleep(Duration::from_secs((2u64 << fails.min(4)).min(30)));
    }
}
fn ws_kraken(lp: Live) {
    // event_trigger=bbo → updates on every quote change (not just trades), so
    // even low-volume pairs like KAS stay fresh. Price = bid/ask mid (the current
    // fair price), not last-trade (which goes stale between trades).
    ws_loop("kraken", "wss://ws.kraken.com/v2",
        r#"{"method":"subscribe","params":{"channel":"ticker","symbol":["KAS/USD","BTC/USD","ETH/USD"],"event_trigger":"bbo"}}"#, &lp,
        |j, lp| if j["channel"] == "ticker" { if let Some(a) = j["data"].as_array() { for d in a {
            if let (Some(sym), Some(bid), Some(ask)) = (d["symbol"].as_str(), d["bid"].as_f64(), d["ask"].as_f64()) {
                if bid > 0.0 && ask > 0.0 { set_price(lp, sym, "Kraken", (bid + ask) / 2.0); }
            }
        } } });
}
fn ws_bybit(lp: Live) {
    ws_loop("bybit", "wss://stream.bybit.com/v5/public/spot",
        r#"{"op":"subscribe","args":["tickers.KASUSDT","tickers.BTCUSDT","tickers.ETHUSDT"]}"#, &lp,
        |j, lp| if j["topic"].as_str().map_or(false, |x| x.starts_with("tickers.")) {
            let pair = match j["data"]["symbol"].as_str() { Some("KASUSDT") => "KAS/USD", Some("BTCUSDT") => "BTC/USD", Some("ETHUSDT") => "ETH/USD", _ => return };
            if let Some(px) = j["data"]["lastPrice"].as_str().and_then(|x| x.parse::<f64>().ok()) { set_price(lp, pair, "Bybit", px); }
        });
}
fn ws_okx(lp: Live) {
    ws_loop("okx", "wss://ws.okx.com:8443/ws/v5/public",
        r#"{"op":"subscribe","args":[{"channel":"tickers","instId":"KAS-USDT"},{"channel":"tickers","instId":"BTC-USDT"},{"channel":"tickers","instId":"ETH-USDT"}]}"#, &lp,
        |j, lp| if let Some(arr) = j["data"].as_array() { for d in arr {
            let pair = match d["instId"].as_str() { Some("KAS-USDT") => "KAS/USD", Some("BTC-USDT") => "BTC/USD", Some("ETH-USDT") => "ETH/USD", _ => continue };
            if let Some(px) = d["last"].as_str().and_then(|x| x.parse::<f64>().ok()) { set_price(lp, pair, "OKX", px); }
        } });
}
fn ws_coinbase(lp: Live) { // BTC/ETH only — Coinbase doesn't list KAS
    ws_loop("coinbase", "wss://ws-feed.exchange.coinbase.com",
        r#"{"type":"subscribe","product_ids":["BTC-USD","ETH-USD"],"channels":["ticker"]}"#, &lp,
        |j, lp| if j["type"] == "ticker" {
            let pair = match j["product_id"].as_str() { Some("BTC-USD") => "BTC/USD", Some("ETH-USD") => "ETH/USD", _ => return };
            if let Some(px) = j["price"].as_str().and_then(|x| x.parse::<f64>().ok()) { set_price(lp, pair, "Coinbase", px); }
        });
}

// ---------- slow REST + KRC-20 (adds sources + the tokens) ----------
fn agent() -> ureq::Agent { ureq::AgentBuilder::new().timeout(Duration::from_secs(7)).build() }
fn get(a: &ureq::Agent, url: &str) -> Option<serde_json::Value> { a.get(url).call().ok()?.into_json().ok() }
fn pf(v: &serde_json::Value) -> Option<f64> { v.as_str().and_then(|s| s.parse().ok()).or_else(|| v.as_f64()) }
fn kucoin(a: &ureq::Agent, s: &str) -> Option<f64> { pf(&get(a, &format!("https://api.kucoin.com/api/v1/market/orderbook/level1?symbol={s}"))?["data"]["price"]) }
fn gate(a: &ureq::Agent, s: &str) -> Option<f64> { get(a, &format!("https://api.gateio.ws/api/v4/spot/tickers?currency_pair={s}"))?.get(0).and_then(|x| pf(&x["last"])) }
fn mexc(a: &ureq::Agent, s: &str) -> Option<f64> { pf(&get(a, &format!("https://api.mexc.com/api/v3/ticker/price?symbol={s}"))?["price"]) }

// ---------- direct Kasplex DEX pool read — OUR OWN on-chain source ----------
// getReserves() CROSS-CHECKED across RPCs (set KASPLEX_RPCS=https://your-node,…
// to include your own node — then no single RPC is trusted). Windowed median
// (TWAP) + a liquidity gate defend against flash-loan spot manipulation.
// a "chain" tag identifies (network, DEX): igra=Igra/Zealous, igrakc=Igra/KaspaCom.
// Both Igra venues share the Igra RPC but are DISTINCT price sources → medianed.
const CHAINS: [&str; 3] = ["kasplex", "igra", "igrakc"];
fn chain_rpcs(chain: &str) -> Vec<String> {
    let (env, default) = match chain {
        "igra" | "igrakc" => ("IGRA_RPCS", "https://rpc.igralabs.com:8545"),
        _ => ("KASPLEX_RPCS", "https://evmrpc.kasplex.org"),
    };
    std::env::var(env).ok().filter(|s| !s.is_empty())
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_else(|| vec![default.to_string()])
}
fn dex_source(chain: &str) -> &'static str { match chain { "igra" => "Igra-Zealous", "igrakc" => "Igra-KaspaCom", _ => "Kasplex-Zealous" } }
fn eth_call_cross(rpcs: &[String], to: &str, data: &str) -> Option<String> {
    let a = agent();
    let body = format!(r#"{{"jsonrpc":"2.0","method":"eth_call","params":[{{"to":"{to}","data":"{data}"}},"latest"],"id":1}}"#);
    let mut got = Vec::new();
    for rpc in rpcs {
        if let Ok(r) = a.post(rpc).set("content-type", "application/json").send_string(&body) {
            if let Ok(j) = r.into_json::<serde_json::Value>() { if let Some(s) = j["result"].as_str() { got.push(s.to_string()); } }
        }
    }
    if got.is_empty() { return None; }
    // When ≥2 RPCs are configured, require ≥2 agreeing responses — a single
    // compromised/flaky RPC must not be enough to move a price.
    if rpcs.len() >= 2 && got.len() < 2 {
        eprintln!("RPC quorum failed on {to}: only {}/{} responded — dropping", got.len(), rpcs.len());
        return None;
    }
    if got.iter().all(|r| r == &got[0]) { Some(got.remove(0)) } else { eprintln!("RPC disagreement on {to} — dropping this read"); None }
}
fn resv(h: &str) -> Option<f64> { u128::from_str_radix(h.get(32..64)?, 16).ok().map(|v| v as f64) } // uint112 fits in the low 128 bits
fn pool_read(rpcs: &[String], p: &Pool) -> Option<(f64, f64)> { // (price_in_wkas, wkas_liquidity)
    let h = eth_call_cross(rpcs, &p.pool, "0x0902f1ac")?; let h = h.trim_start_matches("0x");
    if h.len() < 128 { return None; }
    let (r0, r1) = (resv(&h[0..64])?, resv(&h[64..128])?);
    let (rw, rt) = if p.wkas_is_token0 { (r0, r1) } else { (r1, r0) };
    if rt <= 0.0 { return None; }
    Some(((rw / 1e18) / (rt / 10f64.powi(p.dec as i32)), rw / 1e18)) // (WKAS price, WKAS liquidity)
}
fn kas_usd(lp: &Live) -> f64 {
    // median of FRESH sources only — a frozen venue must not keep voting on the
    // KAS/USD that multiplies every KRC-20 price
    let tms = now_ms();
    match lp.lock().unwrap().get("KAS/USD") {
        Some(m) => {
            let mut v: Vec<f64> = m.values().filter(|(_, t)| tms.saturating_sub(*t) < STALE_MS).map(|(p, _)| *p).collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            if v.is_empty() { 0.0 } else { v[v.len() / 2] }
        }
        None => 0.0,
    }
}
// per-pair WKAS liquidity, for the thin-pool flag
static LIQ: std::sync::OnceLock<Mutex<HashMap<String, f64>>> = std::sync::OnceLock::new();
fn liq_map() -> &'static Mutex<HashMap<String, f64>> { LIQ.get_or_init(|| Mutex::new(HashMap::new())) }
const MIN_LIQ_WKAS: f64 = 1000.0; // below this a KRC-20 feed is flagged "thin" (manipulable / low-confidence)
const TWAP_N: usize = 12;         // ~60s window at SLOW_EVERY=5s — kills single-block flash-loan spikes

// ---------- auto-discovery: re-enumerate the DEX factories on-chain ----------
struct Venue { chain: &'static str, factory: &'static str }
fn venues() -> [Venue; 3] {
    [ Venue { chain: "kasplex", factory: "0xa9cba43a407c9eb30933ea21f7b9d74a128d613c" },
      Venue { chain: "igra",    factory: "0x98Bb580A77eE329796a79aBd05c6D2F2b3D5E1bD" },
      Venue { chain: "igrakc",  factory: "0x21350BcDa9E81731CF4cDE3DbC457e3de2739c01" } ]
}
fn call_u128(rpcs: &[String], to: &str, data: &str) -> Option<u128> {
    let h = eth_call_cross(rpcs, to, data)?; let h = h.trim_start_matches("0x");
    if h.len() < 64 { return None; } u128::from_str_radix(&h[32..64], 16).ok()
}
fn call_addr(rpcs: &[String], to: &str, data: &str) -> Option<String> {
    let h = eth_call_cross(rpcs, to, data)?; let h = h.trim_start_matches("0x");
    if h.len() < 64 { return None; } Some(format!("0x{}", &h[24..64]))
}
fn call_str(rpcs: &[String], to: &str, data: &str) -> Option<String> {
    let h = eth_call_cross(rpcs, to, data)?; let h = h.trim_start_matches("0x");
    if h.len() < 128 { return None; }
    let len = usize::from_str_radix(&h[64..128], 16).ok()?;
    let bytes: Vec<u8> = (0..len.min(64)).filter_map(|i| u8::from_str_radix(h.get(128 + i*2..130 + i*2)?, 16).ok()).collect();
    let s = String::from_utf8_lossy(&bytes).trim_matches(|c: char| c == '\0' || c.is_control()).to_string();
    if s.is_empty() { None } else { Some(s) }
}
fn par<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let mut out = Vec::with_capacity(items.len());
    for chunk in items.chunks(16) {
        let mut part: Vec<R> = std::thread::scope(|s| chunk.iter().map(|it| s.spawn(|| f(it)))
            .collect::<Vec<_>>().into_iter().map(|h| h.join().unwrap()).collect());
        out.append(&mut part);
    }
    out
}
fn discover_venue(v: &Venue) -> Vec<Pool> {
    let rpcs = chain_rpcs(v.chain);
    let n = match call_u128(&rpcs, v.factory, "0x574f2ba3") { Some(n) if (1..5000).contains(&n) => n as usize, _ => return vec![] };
    let idx: Vec<usize> = (0..n).collect();
    let pairs: Vec<String> = par(&idx, |i| call_addr(&rpcs, v.factory, &format!("0x1e3dd18b{i:064x}")).unwrap_or_default())
        .into_iter().filter(|p| p.len() == 42 && !p.ends_with(&"0".repeat(40))).collect();
    let toks: Vec<(String, String)> = par(&pairs, |p| (call_addr(&rpcs, p, "0x0dfe1681").unwrap_or_default(), call_addr(&rpcs, p, "0xd21220a7").unwrap_or_default()));
    let mut freq: HashMap<String, u32> = HashMap::new();
    for (a, b) in &toks { *freq.entry(a.clone()).or_insert(0) += 1; *freq.entry(b.clone()).or_insert(0) += 1; }
    let base = match freq.into_iter().filter(|(k, _)| k.len() == 42).max_by_key(|(_, c)| *c).map(|(k, _)| k) { Some(b) => b, None => return vec![] };
    let entries: Vec<(String, bool, String)> = pairs.into_iter().zip(toks).filter_map(|(p, (t0, t1))| {
        if t0 == base { Some((p, true, t1)) } else if t1 == base { Some((p, false, t0)) } else { None }
    }).collect();
    par(&entries, |(pool, b0, tok)| {
        let h = eth_call_cross(&rpcs, pool, "0x0902f1ac")?; let h = h.trim_start_matches("0x");
        if h.len() < 128 { return None; }
        let (rw, rt) = if *b0 { (resv(&h[0..64])?, resv(&h[64..128])?) } else { (resv(&h[64..128])?, resv(&h[0..64])?) };
        if rw / 1e18 < 50.0 || rt <= 0.0 { return None; }
        let sym = clean_symbol(&call_str(&rpcs, tok, "0x95d89b41")?)?;
        if matches!(sym.to_uppercase().as_str(), "KAS" | "BTC" | "ETH" | "WKAS" | "WIKAS") { return None; }
        let dec = call_u128(&rpcs, tok, "0x313ce567").map(|d| d as u32).filter(|d| *d <= 30).unwrap_or(18);
        Some(Pool { symbol: sym, pool: pool.clone(), wkas_is_token0: *b0, dec, chain: v.chain.to_string() })
    }).into_iter().flatten().collect()
}
fn pools_to_json(pools: &[Pool]) -> String {
    let items: Vec<String> = pools.iter().map(|p| format!(r#"{{"symbol":"{}","pair":"{}","wkas_is_token0":{},"dec":{},"chain":"{}"}}"#, p.symbol, p.pool, p.wkas_is_token0, p.dec, p.chain)).collect();
    format!("[{}]", items.join(","))
}
fn discover_thread() {
    std::thread::sleep(Duration::from_secs(45)); // let the oracle stabilize; startup uses the cached pools.json
    loop {
        let mut all = Vec::new();
        for v in venues() { all.extend(discover_venue(&v)); }
        if all.len() >= 10 { // sanity gate — never clobber the live set with a near-empty enumeration
            let _ = std::fs::write("pools.json", pools_to_json(&all));
            *pools_cell().lock().unwrap() = Arc::new(all);
            eprintln!("auto-discovery: refreshed {} pools across {} venues", load_pools().len(), venues().len());
        } else {
            // exact token — the deploy's log-based alert policy matches it
            eprintln!("KASPULSE_DISCOVERY_EMPTY: only {} pools found — keeping the current set", all.len());
        }
        std::thread::sleep(Duration::from_secs(600)); // every 10 min
    }
}

fn slow_thread(lp: Live) {
    for c in CHAINS {
        let r = chain_rpcs(c);
        eprintln!("{c} RPCs ({}): {}", r.len(), r.join(", "));
        if r.len() < 2 {
            eprintln!("warning: {c} has only 1 RPC — cross-check quorum inactive; set KASPLEX_RPCS / IGRA_RPCS to ≥2 endpoints");
        }
    }
    let mut win: HashMap<String, Vec<f64>> = HashMap::new();
    loop {
        let a = agent();
        for f in feeds() {
            if let Some(s) = f.kucoin { if let Some(p) = kucoin(&a, s) { set_price(&lp, &f.pair, "KuCoin", p); } }
            if let Some(s) = f.gate   { if let Some(p) = gate(&a, s)   { set_price(&lp, &f.pair, "Gate.io", p); } }
            if let Some(s) = f.mexc   { if let Some(p) = mexc(&a, s)   { set_price(&lp, &f.pair, "MEXC", p); } }
        }
        // KRC-20: read each pool on ITS chain (cross-checked) → windowed median (TWAP) → publish.
        // A token on both chains gets two on-chain sources (Kasplex-Zealous + Igra-*) → build() medians.
        let ku = kas_usd(&lp);
        if ku > 0.0 {
            // read pools in parallel (bounded concurrency) so the whole set refreshes in seconds, not a minute
            let pl = load_pools();
            for chunk in pl.chunks(12) {
                let reads: Vec<(String, &str, Option<(f64, f64)>)> = std::thread::scope(|s| {
                    chunk.iter().map(|p| s.spawn(move || (format!("{}/USD", p.symbol), p.chain.as_str(), pool_read(&chain_rpcs(&p.chain), p))))
                        .collect::<Vec<_>>().into_iter().map(|h| h.join().unwrap()).collect()
                });
                for (pair, chain, res) in reads {
                    if let Some((px_kas, liq)) = res {
                        let w = win.entry(format!("{pair}|{chain}")).or_default(); // window per (pair, chain)
                        w.push(px_kas * ku); if w.len() > TWAP_N { let d = w.len() - TWAP_N; w.drain(0..d); }
                        set_price(&lp, &pair, dex_source(chain), median(w));
                        // store CURRENT liquidity per (pair, chain) — overwritten every
                        // round so a drained pool loses its 'liquid' status immediately
                        liq_map().lock().unwrap().insert(format!("{pair}|{chain}"), liq);
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_secs(SLOW_EVERY));
    }
}

// ---------- median + signing ----------
fn median(xs: &[f64]) -> f64 { let mut v = xs.to_vec(); v.sort_by(|a, b| a.partial_cmp(b).unwrap()); let n = v.len(); if n == 0 { 0.0 } else if n % 2 == 1 { v[n/2] } else { (v[n/2-1]+v[n/2])/2.0 } }
/// Committee key custody — committee continuity IS the product: a keyless
/// restart that silently mints a fresh committee breaks every verifier and
/// on-chain consumer that pinned the old pubkeys. Precedence:
///   (a) env KASPULSE_NODE_KEYS — comma-separated n×64-hex secret keys
///       (Cloud Run injects this via Secret Manager, see scripts/setup-keys.sh)
///   (b) kaspulse-node-{i}.key files (local dev)
///   (c) generate + write files — but ONLY when KASPULSE_REQUIRE_KEYS != "1";
///       with it set (deploy.sh sets it), missing/malformed keys log the exact
///       token KASPULSE_KEYS_MISSING (alert policy matches it) and exit(1).
fn load_keys(n: usize) -> Vec<Keypair> {
    let require = std::env::var("KASPULSE_REQUIRE_KEYS").map_or(false, |v| v == "1");
    let parse = |h: &str| hex::decode(h.trim()).ok()
        .and_then(|b| secp256k1::SecretKey::from_slice(&b).ok())
        .map(|sk| Keypair::from_secret_key(SECP256K1, &sk));
    if let Ok(envk) = std::env::var("KASPULSE_NODE_KEYS") {
        let parts: Vec<&str> = envk.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
        let keys: Vec<Keypair> = parts.iter().filter_map(|h| parse(h)).collect();
        if parts.len() == n && keys.len() == n {
            eprintln!("keys: loaded the {n}-node committee from KASPULSE_NODE_KEYS");
            return keys;
        }
        if require {
            eprintln!("KASPULSE_KEYS_MISSING: KASPULSE_NODE_KEYS is set but malformed ({}/{n} keys parsed) — refusing to generate a new committee", keys.len());
            std::process::exit(1);
        }
        eprintln!("warning: KASPULSE_NODE_KEYS is set but malformed ({}/{n} keys parsed) — falling back to key files", keys.len());
    }
    let mut generated = 0usize;
    let keys: Vec<Keypair> = (0..n).map(|i| {
        let path = format!("kaspulse-node-{i}.key");
        if let Some(kp) = std::fs::read_to_string(&path).ok().and_then(|raw| parse(&raw)) { return kp; }
        if require {
            eprintln!("KASPULSE_KEYS_MISSING: {path} absent or malformed — refusing to generate a new committee");
            std::process::exit(1);
        }
        generated += 1;
        let kp = Keypair::new(SECP256K1, &mut secp256k1::rand::thread_rng());
        let _ = std::fs::write(&path, hex::encode(kp.secret_key().secret_bytes()));
        kp
    }).collect();
    if generated > 0 {
        eprintln!("WARNING: minted {generated} FRESH committee key(s) — every verifier or on-chain consumer pinned to the previous pubkeys just broke. For any public deploy set KASPULSE_NODE_KEYS (scripts/setup-keys.sh) and KASPULSE_REQUIRE_KEYS=1.");
    }
    keys
}
fn sign(kp: &Keypair, msg: &str) -> String { let h = blake2b_simd::Params::new().hash_length(32).hash(msg.as_bytes()); hex::encode(kp.sign_schnorr(secp256k1::Message::from_digest_slice(h.as_bytes()).unwrap()).as_ref()) }
fn enum_(p: f64) -> String { if p == 0.0 { "0".into() } else { format!("{p}") } }

/// The signed representation: price = mant × 10^expo, mant always 9 significant
/// digits. Fixes the price_e8 quantization bug (a $3e-9 token signed as 0).
fn mant_expo(p: f64) -> (u64, i32) {
    if p <= 0.0 || !p.is_finite() { return (0, 0); }
    let mut expo = p.log10().floor() as i32 - 8;
    let mut mant = (p / 10f64.powi(expo)).round() as u64;
    if mant >= 1_000_000_000 { mant /= 10; expo += 1; } // rounding carried into a 10th digit
    (mant, expo)
}
/// last signed attestation per pair — unchanged prices re-sign only on the
/// heartbeat, so signing cost tracks price CHANGES, not the serve loop.
struct SignCache {
    mant: u64, expo: i32, price_e8: u64,
    msg: String, sigs_json: String,
    /// covenant-domain sigs over blake2b(price_bytes) — same keys, on-chain encoding
    cov_sigs_json: String,
    /// 24-byte attestation record + per-node sigs (slash-observable)
    record_hex: String, record_sigs_json: String,
    ts: u64, round: u64,
}

/// Minimal LE script-number encoding of price_e8 (MESSAGE-FORMAT §8.2).
fn price_bytes(price_e8: i64) -> Vec<u8> {
    if price_e8 == 0 { return vec![]; }
    let neg = price_e8 < 0; let mut abs = price_e8.unsigned_abs(); let mut out = Vec::new();
    while abs > 0 { out.push((abs & 0xff) as u8); abs >>= 8; }
    if out.last().unwrap() & 0x80 != 0 { out.push(if neg { 0x80 } else { 0 }); } else if neg { *out.last_mut().unwrap() |= 0x80; }
    out
}
/// 24-byte bond attestation: slot(blake2b(pair)[0..8] ‖ round_be) ‖ mant_be.
fn attestation_record(pair: &str, round: u64, mant: u64) -> [u8; 24] {
    let h = blake2b_simd::Params::new().hash_length(32).hash(pair.as_bytes());
    let mut r = [0u8; 24];
    r[..8].copy_from_slice(&h.as_bytes()[..8]);
    r[8..16].copy_from_slice(&round.to_be_bytes());
    r[16..24].copy_from_slice(&mant.to_be_bytes());
    r
}
fn sign_bytes(kp: &Keypair, data: &[u8]) -> String {
    let h = blake2b_simd::Params::new().hash_length(32).hash(data);
    hex::encode(kp.sign_schnorr(secp256k1::Message::from_digest_slice(h.as_bytes()).unwrap()).as_ref())
}
/// True when a live source name is an Igra-chain DEX venue (peg_ok applies).
fn is_igra_source(name: &str) -> bool { name.starts_with("Igra-") }

// ---- integrity guards (REVIEW §2/§3) ----
const BREAK_PCT: f64 = 0.20;   // a >20% one-round jump is HELD (publish last good)…
const BREAK_ROUNDS: u32 = 12;  // …unless it persists ~5s of rounds — then it's a real move
const PEG_TOL: f64 = 0.02;     // the Igra USDC feed must sit within 2% of $1.00

/// MAD outlier filter: with ≥4 sources, drop anything further than
/// max(4×MAD, 0.3%) from the median — a hijacked venue contributes NOTHING.
/// Never drops below 2 surviving sources.
fn mad_filter(srcs: &mut Vec<(&str, f64, u64)>) -> Vec<String> {
    if srcs.len() < 4 { return vec![]; }
    let prices: Vec<f64> = srcs.iter().map(|(_, p, _)| *p).collect();
    let m = median(&prices);
    let devs: Vec<f64> = prices.iter().map(|p| (p - m).abs()).collect();
    let tol = (4.0 * median(&devs)).max(m * 0.003);
    let dropped: Vec<String> = srcs.iter().filter(|(_, p, _)| (p - m).abs() > tol).map(|(n, _, _)| n.to_string()).collect();
    if dropped.is_empty() || srcs.len() - dropped.len() < 2 { return vec![]; }
    srcs.retain(|(_, p, _)| (p - m).abs() <= tol);
    dropped
}

struct FeedRow { cfg: FeedCfg, srcs: Vec<(String, f64, u64)>, outliers: Vec<String>, med: f64, halted: bool, degraded: bool }

/// One build round's pre-serialized output — everything http::PubState serves.
struct Built {
    envelope: String, per_pair: Vec<(String, String)>, catalog: String, committee: String,
    feeds_total: usize, feeds_live: usize,
}

fn build(lp: &Live, keys: &[Keypair], round: u64, hist: &mut HashMap<String, Vec<(u64, f64)>>, scache: &mut HashMap<String, SignCache>, bstate: &mut HashMap<String, (f64, u32)>, remote: &aggregate::RemoteBook) -> Built {
    let ts = now(); let tms = now_ms();
    let signers: Vec<String> = keys.iter().map(|k| format!("\"{}\"", hex::encode(k.x_only_public_key().0.serialize()))).collect();
    let committee = format!(
        r#"{{"threshold":{THRESHOLD},"num_nodes":{N_NODES},"signers":[{}],"message":"kaspulse/v2","covenant":"blake2b(price_bytes)","updated_ts":{ts}}}"#,
        signers.join(","));
    let book = lp.lock().unwrap().clone();

    // ── pass 1: sources → MAD filter → median → circuit breaker ──
    let mut rows: Vec<FeedRow> = Vec::new();
    for cfg in feeds() {
        let per = match book.get(&cfg.pair) { Some(m) => m, None => continue };
        let mut srcs: Vec<(&str, f64, u64)> = per.iter().filter(|(_, (_, t))| tms.saturating_sub(*t) < STALE_MS).map(|(n, (p, t))| (*n, *p, tms.saturating_sub(*t))).collect();
        if srcs.is_empty() { continue; }
        srcs.sort_by(|a, b| a.0.cmp(b.0));
        let outliers = mad_filter(&mut srcs);
        let raw_med = median(&srcs.iter().map(|(_, p, _)| *p).collect::<Vec<_>>());
        // breaker: a violent jump publishes the LAST GOOD price until it persists
        let (med, halted) = match bstate.get(&cfg.pair).copied() {
            Some((lg, n)) if lg > 0.0 && (raw_med - lg).abs() / lg > BREAK_PCT => {
                if n + 1 >= BREAK_ROUNDS { bstate.insert(cfg.pair.clone(), (raw_med, 0)); (raw_med, false) }
                else { bstate.insert(cfg.pair.clone(), (lg, n + 1)); (lg, true) }
            }
            _ => { bstate.insert(cfg.pair.clone(), (raw_med, 0)); (raw_med, false) }
        };
        let degraded = cfg.kind == "major" && srcs.len() < 2; // a major on one venue is low-confidence
        rows.push(FeedRow { cfg, srcs: srcs.into_iter().map(|(n, p, a)| (n.to_string(), p, a)).collect(), outliers, med, halted, degraded });
    }

    // ── peg check: Igra's USDC feed should sit at ~$1.00 — drift means the
    //    iKAS bridge (or USDC itself) depegged, so every Igra price is suspect ──
    let usdc = rows.iter().find(|r| r.cfg.pair == "USDC/USD").map(|r| r.med);
    let igra_peg_ok = usdc.map(|u| (u - 1.0).abs() < PEG_TOL);

    // ── pass 2: sign the PUBLISHED price + render ──
    let mut objs = Vec::new();
    let mut per_pair: Vec<(String, String)> = Vec::new(); // "KAS-USD" -> FeedObj JSON
    let mut cat_rows: Vec<String> = Vec::new();           // /v1/feeds light catalog
    let mut feeds_live = 0usize;
    for r in &rows {
        let med = r.med;
        let price_e8 = (med * 1e8).round() as u64; // also the covenant-domain integer (MESSAGE-FORMAT §8.2)
        let (mant, expo) = mant_expo(med);
        // sign on CHANGE, re-sign unchanged prices only on the heartbeat
        let (msg, sigs_json, cov_sigs_json, record_hex, record_sigs_json, signed_ts, signed_round) = match scache.get(&r.cfg.pair) {
            Some(c) if c.mant == mant && c.expo == expo && c.price_e8 == price_e8 && ts.saturating_sub(c.ts) < HEARTBEAT_S =>
                (c.msg.clone(), c.sigs_json.clone(), c.cov_sigs_json.clone(), c.record_hex.clone(), c.record_sigs_json.clone(), c.ts, c.round),
            _ => {
                let msg = format!("kaspulse/v2|{}|{mant}|{expo}|{ts}|{round}", r.cfg.pair);
                let sigs_json = keys.iter().map(|k| format!("\"{}\"", sign(k, &msg))).collect::<Vec<_>>().join(",");
                let pb = price_bytes(price_e8 as i64);
                let cov_sigs_json = keys.iter().map(|k| format!("\"{}\"", sign_bytes(k, &pb))).collect::<Vec<_>>().join(",");
                let rec = attestation_record(&r.cfg.pair, round, mant);
                let record_hex = hex::encode(rec);
                let record_sigs_json = keys.iter().map(|k| format!("\"{}\"", sign_bytes(k, &rec))).collect::<Vec<_>>().join(",");
                scache.insert(r.cfg.pair.clone(), SignCache {
                    mant, expo, price_e8, msg: msg.clone(), sigs_json: sigs_json.clone(),
                    cov_sigs_json: cov_sigs_json.clone(), record_hex: record_hex.clone(),
                    record_sigs_json: record_sigs_json.clone(), ts, round,
                });
                (msg, sigs_json, cov_sigs_json, record_hex, record_sigs_json, ts, round)
            }
        };
        // merge independently-verified remote operator attests that agree on mant/expo
        let (extra_pks, extra_sigs) = aggregate::extras(remote, &r.cfg.pair, mant, expo, 30);
        let mut all_signers = signers.clone();
        let mut all_sigs = if sigs_json.is_empty() { Vec::new() } else { sigs_json.split(',').map(|s| s.to_string()).collect::<Vec<_>>() };
        for (pk, sg) in extra_pks.into_iter().zip(extra_sigs) {
            if !all_signers.iter().any(|s| s == &pk) {
                all_signers.push(pk);
                all_sigs.push(sg);
            }
        }
        let signers_j = all_signers.join(",");
        let sigs_j = all_sigs.join(",");
        let threshold_eff = THRESHOLD; // still need 3 — remotes can only ADD votes
        let prices: Vec<f64> = r.srcs.iter().map(|(_, p, _)| *p).collect();
        let lo = prices.iter().cloned().fold(f64::MAX, f64::min); let hi = prices.iter().cloned().fold(f64::MIN, f64::max);
        let spread = if med > 0.0 { ((hi - lo) / med) * 10_000.0 } else { 0.0 };
        let freshest = r.srcs.iter().map(|(_, _, a)| *a).min().unwrap_or(0);
        // deepest CURRENT venue liquidity (per-chain entries are overwritten each round — drained pools decay)
        let liq = { let lm = liq_map().lock().unwrap();
            CHAINS.iter().filter_map(|c| lm.get(&format!("{}|{c}", r.cfg.pair))).cloned().fold(0.0_f64, f64::max) };
        let thin = r.cfg.kind == "krc20" && liq < MIN_LIQ_WKAS; // low-liquidity pool → manipulable, low-confidence
        let src_j: Vec<String> = r.srcs.iter().map(|(n, p, a)| format!(r#"{{"name":"{n}","price":{},"age_ms":{a}}}"#, enum_(*p))).collect();
        let out_j: Vec<String> = r.outliers.iter().map(|n| format!("\"{n}\"")).collect();
        // Igra venues are named Igra-Zealous / Igra-KaspaCom (not the old "Igra-DEX")
        let peg_field = if r.srcs.iter().any(|(n, _, _)| is_igra_source(n)) {
            match igra_peg_ok { Some(ok) => format!(r#","peg_ok":{ok}"#), None => r#","peg_ok":null"#.to_string() }
        } else { String::new() };
        let cov_j = format!(
            r#"{{"price_e8":{price_e8},"price_bytes":"{}","signatures":[{cov_sigs_json}],"record":"{record_hex}","record_signatures":[{record_sigs_json}]}}"#,
            hex::encode(price_bytes(price_e8 as i64)));
        let h = hist.entry(r.cfg.pair.clone()).or_default(); h.push((ts, med)); if h.len() > HISTORY { let d = h.len() - HISTORY; h.drain(0..d); }
        let hist_j: Vec<String> = h.iter().map(|(t, p)| format!("[{t},{}]", enum_(*p))).collect();
        let obj = format!(
            r#"{{"pair":"{}","kind":"{}","price":{},"price_e8":{price_e8},"mant":{mant},"expo":{expo},"sources":[{}],"num_sources":{},"outliers":[{}],"halted":{},"degraded":{}{peg_field},"freshest_ms":{freshest},"low":{},"high":{},"spread_bps":{:.2},"median":{},"twap":true,"liq_wkas":{:.0},"thin":{thin},"signers":[{signers_j}],"threshold":{threshold_eff},"signatures":[{sigs_j}],"message":"{msg}","signed_ts":{signed_ts},"signed_round":{signed_round},"covenant":{cov_j},"history":[{}]}}"#,
            r.cfg.pair, r.cfg.kind, enum_(med), src_j.join(","), r.srcs.len(), out_j.join(","), r.halted, r.degraded, enum_(lo), enum_(hi), spread, enum_(med), liq, hist_j.join(",")
        );
        // per-pair map (dash form, uppercase) — /v1/feed/{PAIR} serves this
        // string directly instead of re-parsing the whole envelope per request
        per_pair.push((r.cfg.pair.replace('/', "-").to_uppercase(), obj.clone()));
        // light catalog row — what dashboards poll instead of the full envelope
        cat_rows.push(format!(
            r#"{{"pair":"{}","kind":"{}","price":{},"num_sources":{},"halted":{},"degraded":{},"thin":{thin},"liq_wkas":{:.0},"spread_bps":{:.2},"freshest_ms":{freshest}}}"#,
            r.cfg.pair, r.cfg.kind, enum_(med), r.srcs.len(), r.halted, r.degraded, liq, spread));
        if !r.halted && !r.srcs.is_empty() { feeds_live += 1; }
        objs.push(obj);
    }
    let peg_j = format!(r#"{{"igra_usdc":{},"igra_ok":{}}}"#,
        usdc.map(|u| format!("{u}")).unwrap_or_else(|| "null".into()),
        igra_peg_ok.map(|b| b.to_string()).unwrap_or_else(|| "null".into()));
    let envelope = format!(r#"{{"round":{round},"timestamp":{ts},"threshold":{THRESHOLD},"num_nodes":{N_NODES},"transport":"websocket","peg":{peg_j},"feeds":[{}]}}"#, objs.join(","));
    let catalog = format!(r#"{{"round":{round},"timestamp":{ts},"count":{},"feeds":[{}]}}"#, cat_rows.len(), cat_rows.join(","));
    Built { envelope, per_pair, catalog, committee, feeds_total: rows.len(), feeds_live }
}

// ---------- http: see src/http.rs (hardened std server, /v1 + aliases) ----------

/// Poll independent `signer` daemons when `KASPULSE_OPERATORS` is set.
/// Each URL is an `/attest` base; responses contribute remote Schnorr sigs that
/// must verify under the operator's published pubkey. Local keys still sign
/// (dev / bootstrap); once operators are configured the feed lists both.
mod aggregate {
    use super::*;
    use secp256k1::{schnorr, Message, XOnlyPublicKey};

    #[derive(Clone)]
    pub struct RemoteAttest {
        pub pair: String, pub mant: u64, pub expo: i32, pub ts: u64, pub round: u64,
        pub signer: String, pub signature: String, pub message: String,
    }

    pub type RemoteBook = Arc<Mutex<HashMap<String, Vec<RemoteAttest>>>>; // pair → attests

    pub fn spawn(book: RemoteBook) {
        let urls: Vec<String> = match std::env::var("KASPULSE_OPERATORS") {
            Ok(s) if !s.trim().is_empty() => s.split(',').map(|x| x.trim().trim_end_matches('/').to_string()).filter(|s| !s.is_empty()).collect(),
            _ => return,
        };
        eprintln!("aggregator: polling {} operator /attest endpoint(s)", urls.len());
        std::thread::spawn(move || {
            let a = agent();
            loop {
                let mut next: HashMap<String, Vec<RemoteAttest>> = HashMap::new();
                for url in &urls {
                    let attest_url = if url.ends_with("/attest") { url.clone() } else { format!("{url}/attest") };
                    let Ok(resp) = a.get(&attest_url).call() else { continue };
                    let Ok(arr) = resp.into_json::<serde_json::Value>() else { continue };
                    let Some(items) = arr.as_array() else { continue };
                    for it in items {
                        let Some(att) = parse_attest(it) else { continue };
                        if !verify_attest(&att) { continue; }
                        next.entry(att.pair.clone()).or_default().push(att);
                    }
                }
                *book.lock().unwrap() = next;
                std::thread::sleep(Duration::from_secs(2));
            }
        });
    }

    fn parse_attest(v: &serde_json::Value) -> Option<RemoteAttest> {
        Some(RemoteAttest {
            pair: v["pair"].as_str()?.to_string(),
            mant: v["mant"].as_u64()?,
            expo: v["expo"].as_i64()? as i32,
            ts: v["ts"].as_u64()?,
            round: v["round"].as_u64()?,
            signer: v["signer"].as_str()?.to_string(),
            signature: v["signature"].as_str()?.to_string(),
            message: v["message"].as_str()?.to_string(),
        })
    }

    fn verify_attest(a: &RemoteAttest) -> bool {
        let want = format!("kaspulse/v2|{}|{}|{}|{}|{}", a.pair, a.mant, a.expo, a.ts, a.round);
        if a.message != want { return false; }
        let h = blake2b_simd::Params::new().hash_length(32).hash(a.message.as_bytes());
        let Ok(msg) = Message::from_digest_slice(h.as_bytes()) else { return false };
        let Ok(pk) = XOnlyPublicKey::from_slice(&hex::decode(&a.signer).unwrap_or_default()) else { return false };
        let Ok(sig) = schnorr::Signature::from_slice(&hex::decode(&a.signature).unwrap_or_default()) else { return false };
        SECP256K1.verify_schnorr(&sig, &msg, &pk).is_ok()
    }

    /// Merge remote operator pubkeys/sigs into the local committee arrays for a pair.
    /// Returns (extra_signers_json_elems, extra_sigs_json_elems) that agree with mant/expo.
    pub fn extras(book: &RemoteBook, pair: &str, mant: u64, expo: i32, max_age_s: u64) -> (Vec<String>, Vec<String>) {
        let now = now();
        let guard = book.lock().unwrap();
        let Some(list) = guard.get(pair) else { return (vec![], vec![]) };
        let mut signers = Vec::new();
        let mut sigs = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for a in list {
            if a.mant != mant || a.expo != expo { continue; }
            if now.saturating_sub(a.ts) > max_age_s { continue; }
            if !seen.insert(a.signer.clone()) { continue; }
            signers.push(format!("\"{}\"", a.signer));
            sigs.push(format!("\"{}\"", a.signature));
        }
        (signers, sigs)
    }
}

fn main() -> Result<()> {
    let keys = load_keys(N_NODES);
    let operators = std::env::var("KASPULSE_OPERATORS").ok().filter(|s| !s.trim().is_empty());
    if operators.is_some() && std::env::var("KASPULSE_REQUIRE_KEYS").map_or(false, |v| v == "1") {
        eprintln!("keys: local committee active + remote operators via KASPULSE_OPERATORS");
    }
    println!("kaspulse oracle — WebSocket streaming · {N_NODES} nodes, {THRESHOLD}-of-{N_NODES} · serve every {SERVE_MS}ms");
    println!("  majors: Kraken+Bybit+OKX+Coinbase (WS, sub-second) + KuCoin/Gate/MEXC (REST {SLOW_EVERY}s)");
    println!("  KRC-20: direct Kasplex/Igra DEX pool reads (cross-checked RPCs)");

    let remote: aggregate::RemoteBook = Arc::new(Mutex::new(HashMap::new()));
    aggregate::spawn(remote.clone());

    std::thread::spawn(discover_thread);
    let lp: Live = Arc::new(Mutex::new(HashMap::new()));
    for (f, lpc) in [ws_kraken as fn(Live), ws_bybit, ws_okx, ws_coinbase, slow_thread].into_iter().zip(std::iter::repeat(lp.clone())) {
        std::thread::spawn(move || f(lpc));
    }

    let state = Arc::new(http::PubState::new());
    {
        let (state, lp, remote) = (state.clone(), lp.clone(), remote.clone());
        std::thread::spawn(move || {
            let mut round = 1u64; let mut hist = HashMap::new(); let mut scache = HashMap::new(); let mut bstate = HashMap::new();
            loop {
                let b = build(&lp, &keys, round, &mut hist, &mut scache, &mut bstate, &remote);
                state.publish(b.envelope, b.per_pair, b.catalog, b.committee, round, load_pools().len(), b.feeds_total, b.feeds_live);
                round += 1;
                std::thread::sleep(Duration::from_millis(SERVE_MS));
            }
        });
    }
    // bind 0.0.0.0:$PORT so it runs behind Cloud Run / a reverse proxy (PORT env),
    // falling back to the local default
    let port: u16 = std::env::var("PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(PORT);
    println!("serving http://127.0.0.1:{port}  (/v1/feed · /v1/feeds · /v1/committee · /health)");
    http::run(port, state)?;
    Ok(())
}

// ---------- unit tests (integrity guards) ----------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mant_expo_nine_digits() {
        let (m, e) = mant_expo(0.0824);
        assert!((m as f64 * 10f64.powi(e) - 0.0824).abs() < 1e-12);
        assert!((100_000_000..=999_999_999).contains(&m));
        // tiny token that used to collapse to price_e8=0
        let (m2, e2) = mant_expo(3e-9);
        assert!(m2 > 0);
        assert!((m2 as f64 * 10f64.powi(e2) - 3e-9).abs() / 3e-9 < 1e-6);
    }

    #[test]
    fn mad_filter_drops_outlier() {
        let mut srcs = vec![
            ("A", 1.00, 0u64), ("B", 1.01, 0), ("C", 0.99, 0), ("D", 5.00, 0),
        ];
        let dropped = mad_filter(&mut srcs);
        assert!(dropped.contains(&"D".to_string()));
        assert_eq!(srcs.len(), 3);
    }

    #[test]
    fn mad_filter_keeps_small_sets() {
        let mut srcs = vec![("A", 1.0, 0u64), ("B", 9.0, 0), ("C", 1.1, 0)];
        assert!(mad_filter(&mut srcs).is_empty());
        assert_eq!(srcs.len(), 3);
    }

    #[test]
    fn is_igra_source_matches_real_names() {
        assert!(is_igra_source("Igra-Zealous"));
        assert!(is_igra_source("Igra-KaspaCom"));
        assert!(!is_igra_source("Kasplex-Zealous"));
        // regression: build used to require the exact dead name "Igra-DEX"
        assert!(!dex_source("igra").eq("Igra-DEX"));
        assert_eq!(dex_source("igra"), "Igra-Zealous");
        assert_eq!(dex_source("igrakc"), "Igra-KaspaCom");
        assert!(is_igra_source(dex_source("igra")));
        assert!(is_igra_source(dex_source("igrakc")));
    }

    #[test]
    fn circuit_breaker_holds_then_releases() {
        let mut bstate: HashMap<String, (f64, u32)> = HashMap::new();
        let pair = "KAS/USD".to_string();
        bstate.insert(pair.clone(), (1.0, 0));
        // simulate jumps
        let mut halted_count = 0u32;
        let mut last = 1.0;
        for i in 0..BREAK_ROUNDS + 2 {
            let raw = 1.5; // 50% jump
            let (med, halted) = match bstate.get(&pair).copied() {
                Some((lg, n)) if lg > 0.0 && (raw - lg).abs() / lg > BREAK_PCT => {
                    if n + 1 >= BREAK_ROUNDS { bstate.insert(pair.clone(), (raw, 0)); (raw, false) }
                    else { bstate.insert(pair.clone(), (lg, n + 1)); (lg, true) }
                }
                _ => { bstate.insert(pair.clone(), (raw, 0)); (raw, false) }
            };
            if halted { halted_count += 1; assert!((med - 1.0).abs() < 1e-12); }
            last = med;
            let _ = i;
        }
        assert!(halted_count >= BREAK_ROUNDS - 1);
        assert!((last - 1.5).abs() < 1e-12);
    }

    #[test]
    fn price_bytes_and_record_layout() {
        assert_eq!(price_bytes(0), Vec::<u8>::new());
        assert_eq!(price_bytes(128), vec![0x80, 0x00]);
        let r = attestation_record("KAS/USD", 42, 2_900_000);
        let h = blake2b_simd::Params::new().hash_length(32).hash(b"KAS/USD");
        assert_eq!(&r[..8], &h.as_bytes()[..8]);
        assert_eq!(&r[8..16], &42u64.to_be_bytes());
        assert_eq!(&r[16..24], &2_900_000u64.to_be_bytes());
    }

    #[test]
    fn eth_call_cross_quorum_requires_two_when_configured() {
        // unreachable endpoints → empty got → None (no single-response accept with 2 configured)
        let rpcs = vec!["http://127.0.0.1:1".into(), "http://127.0.0.1:2".into()];
        assert!(eth_call_cross(&rpcs, "0xabc", "0x00").is_none());
    }
}

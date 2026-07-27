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

const ROUND_HWM_PATH: &str = "round.hwm";
const ROUND_HWM_LEASE: u64 = 150; // slots reserved on disk AHEAD of the one being issued (~60s at SERVE_MS=400)

fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() }
fn now_ms() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 }

/// Slot high-water mark, persisted so a clock step (or a restore from a snapshot
/// with a stale clock) can never re-issue a round number we already signed.
///
/// The file is a RESERVATION, not a log: we write `round + ROUND_HWM_LEASE`
/// BEFORE issuing `round`, so the on-disk number is always an UPPER bound on
/// everything ever signed. Flushing lazily *after* the fact is the wrong
/// direction — a crash between flushes leaves a floor BELOW the last published
/// round, and the next process re-signs slots it already signed at different
/// prices, which is exactly the equivocation the bond covenant slashes on.
/// The cost of the lease is skipping ≤150 slots (~60s of round numbers) after a
/// restart. Gaps are free; replays are not.
fn read_round_hwm() -> u64 {
    match std::fs::read_to_string(ROUND_HWM_PATH) {
        Ok(s) => match s.trim().parse::<u64>() {
            Ok(v) => v,
            // a truncated or garbage file means we have NO replay floor this boot.
            // Say it loudly (the deploy's log alert matches the token) instead of
            // silently continuing at 0.
            Err(e) => { eprintln!("KASPULSE_ROUND_HWM_UNREADABLE: {ROUND_HWM_PATH} = {:?} ({e}) — no replay floor this boot", s.trim()); 0 }
        },
        Err(_) => 0, // first boot: no file yet
    }
}
/// tmp + rename, because `fs::write` truncates in place: a crash mid-write would
/// otherwise leave a short file that parses as a LOWER floor than the one it
/// replaced — silently worse than not writing at all.
fn write_round_hwm(hwm: u64) {
    let tmp = format!("{ROUND_HWM_PATH}.tmp");
    if let Err(e) = std::fs::write(&tmp, hwm.to_string()).and_then(|_| std::fs::rename(&tmp, ROUND_HWM_PATH)) {
        eprintln!("round.hwm write failed: {e}");
    }
}

// live price book: pair -> exchange -> (price, ts_ms). WS + REST both write here.
type Live = Arc<Mutex<HashMap<String, HashMap<&'static str, (f64, u64)>>>>;
fn set_price(lp: &Live, pair: &str, ex: &'static str, price: f64) {
    if price > 0.0 { lp.lock().unwrap().entry(pair.to_string()).or_default().insert(ex, (price, now_ms())); }
}
/// Write a price only when this venue's existing quote is older than `max_age_ms`.
/// Lets a REST leg refresh a WebSocket source without ever overwriting a fresher
/// WS tick — and without inventing a second source name for the same exchange
/// (which would fake an extra venue in `num_sources`).
fn set_price_if_older(lp: &Live, pair: &str, ex: &'static str, price: f64, max_age_ms: u64) {
    if price <= 0.0 { return; }
    let tms = now_ms();
    let mut g = lp.lock().unwrap();
    let e = g.entry(pair.to_string()).or_default();
    if e.get(ex).map_or(true, |(_, t)| tms.saturating_sub(*t) >= max_age_ms) { e.insert(ex, (price, tms)); }
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
fn ws_okx(lp: Live) { // BTC/ETH only — OKX has no KAS spot instrument (KAS-USDT returns code 51001)
    ws_loop("okx", "wss://ws.okx.com:8443/ws/v5/public",
        r#"{"op":"subscribe","args":[{"channel":"tickers","instId":"BTC-USDT"},{"channel":"tickers","instId":"ETH-USDT"}]}"#, &lp,
        |j, lp| if let Some(arr) = j["data"].as_array() { for d in arr {
            let pair = match d["instId"].as_str() { Some("BTC-USDT") => "BTC/USD", Some("ETH-USDT") => "ETH/USD", _ => continue };
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
/// Kraken REST ticker → bid/ask mid, the SAME basis as the WS leg.
/// Kraken keys `result` by its own internal pair name (KASUSD, XXBTZUSD, …), so
/// take the first entry rather than guessing it.
fn kraken_rest(a: &ureq::Agent, s: &str) -> Option<f64> {
    let j = get(a, &format!("https://api.kraken.com/0/public/Ticker?pair={s}"))?;
    let r = j["result"].as_object()?.values().next()?.clone();
    let (bid, ask) = (pf(&r["b"][0])?, pf(&r["a"][0])?);
    if bid > 0.0 && ask > 0.0 { Some((bid + ask) / 2.0) } else { None }
}

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
fn pool_read(rpcs: &[String], p: &Pool) -> Option<(f64, f64, Option<u64>)> { // (price_in_wkas, wkas_liquidity, blockTimestampLast)
    let h = eth_call_cross(rpcs, &p.pool, "0x0902f1ac")?; let h = h.trim_start_matches("0x");
    if h.len() < 128 { return None; }
    let (r0, r1) = (resv(&h[0..64])?, resv(&h[64..128])?);
    let (rw, rt) = if p.wkas_is_token0 { (r0, r1) } else { (r1, r0) };
    if rt <= 0.0 { return None; }
    // 3rd word = blockTimestampLast (uint32, right-aligned in h[128..192]). Kept
    // OPTIONAL on purpose: a pair that answers getReserves() with two words still
    // prices fine, it just has no age to publish.
    let bts = h.get(184..192).and_then(|s| u64::from_str_radix(s, 16).ok()).filter(|t| *t > 0);
    Some(((rw / 1e18) / (rt / 10f64.powi(p.dec as i32)), rw / 1e18, bts)) // (WKAS price, WKAS liquidity, touch ts)
}
/// Head-block timestamp of a chain. Pool age MUST be measured against this, not
/// wall clock: Igra's head runs ~700s behind real time, which would add a fake
/// ~12 minutes to the age of every Igra pool. Advisory metadata, so the first
/// RPC that answers is enough — no cross-check quorum (nothing signed uses it).
fn chain_head_ts(a: &ureq::Agent, rpcs: &[String]) -> Option<u64> {
    let body = r#"{"jsonrpc":"2.0","method":"eth_getBlockByNumber","params":["latest",false],"id":1}"#;
    for rpc in rpcs {
        if let Ok(r) = a.post(rpc).set("content-type", "application/json").send_string(body) {
            if let Ok(j) = r.into_json::<serde_json::Value>() {
                if let Some(t) = j["result"]["timestamp"].as_str()
                    .and_then(|h| u64::from_str_radix(h.trim_start_matches("0x"), 16).ok()) { return Some(t); }
            }
        }
    }
    None
}
/// All chain heads for one pool round, DE-DUPLICATED (igra and igrakc share one
/// RPC list) and fetched in PARALLEL on a short timeout.
///
/// This sits in front of the pool sweep, so it is on the price path: fetched
/// serially on the 7s agent, two dead Igra endpoints alone prepend 28s to a round
/// that already runs tens of seconds, pushing every DEX source past STALE_MS and
/// taking all 58 KRC-20 feeds off the air — over a field (pool_age_s) that is
/// unsigned advisory metadata. A chain that doesn't answer inside HEAD_TIMEOUT_S
/// just gets no head, and its pools publish `pool_age_s: null`.
const HEAD_TIMEOUT_S: u64 = 2;
fn chain_heads() -> HashMap<&'static str, u64> {
    let a = ureq::AgentBuilder::new().timeout(Duration::from_secs(HEAD_TIMEOUT_S)).build();
    let mut uniq: Vec<(Vec<String>, Vec<&'static str>)> = Vec::new();
    for c in CHAINS {
        let r = chain_rpcs(c);
        match uniq.iter_mut().find(|(k, _)| *k == r) { Some(e) => e.1.push(c), None => uniq.push((r, vec![c])) }
    }
    let heads: Vec<Option<u64>> = std::thread::scope(|s| {
        uniq.iter().map(|(r, _)| s.spawn(|| chain_head_ts(&a, r))).collect::<Vec<_>>()
            .into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut out = HashMap::new();
    for ((_, cs), h) in uniq.iter().zip(heads) { if let Some(t) = h { for c in cs { out.insert(*c, t); } } }
    out
}
/// KAS/USD as PUBLISHED (post-MAD, post-circuit-breaker), written by build() at
/// the end of pass 1. Every KRC-20 price is a WKAS ratio × this number, so it has
/// to be the guarded one: otherwise a KAS/USD feed publishing halted:true with a
/// held price ships in the SAME envelope as 58 KRC-20 feeds built on the un-held
/// raw median — a self-contradicting document.
///
/// STAMPED, and the stamp is load-bearing: an unbounded cache turns the KRC-20
/// path fail-OPEN. Lose egress to the exchanges for a few minutes and the KAS/USD
/// row drops out of build() entirely, so nothing refreshes this cell — and every
/// KRC-20 feed would keep publishing a freshly-signed, `halted:false` dollar
/// price computed from a frozen KAS quote. Past STALE_MS the cell stops being an
/// answer: kas_usd() falls through to the (also freshness-filtered) cold-start
/// median, which returns 0.0, which makes slow_thread skip the pool round so the
/// KRC-20 sources age out of the book on their own. Fail closed.
static KU: std::sync::OnceLock<Mutex<(f64, u64)>> = std::sync::OnceLock::new();
fn ku_cell() -> &'static Mutex<(f64, u64)> { KU.get_or_init(|| Mutex::new((0.0, 0))) }
fn kas_usd(lp: &Live) -> f64 {
    let (guarded, at) = *ku_cell().lock().unwrap();
    if guarded > 0.0 && now_ms().saturating_sub(at) < STALE_MS { return guarded; }
    // COLD START ONLY (build() has not completed a round yet): median of FRESH
    // sources — a frozen venue must not vote on the KAS/USD that multiplies every
    // KRC-20 price. Unguarded, hence the fallback and not the normal path.
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
// Per-(pair|chain) venue state from the last SUCCESSFUL pool read, each entry
// STAMPED with its read time. The stamp is the point: these maps are only ever
// written on success and never deleted, so without it a pool that drained (or
// whose RPC blipped) would keep its liquidity — and its "not thin" badge —
// forever. Every reader drops entries older than STALE_MS.
static LIQ: std::sync::OnceLock<Mutex<HashMap<String, (f64, u64)>>> = std::sync::OnceLock::new();
fn liq_map() -> &'static Mutex<HashMap<String, (f64, u64)>> { LIQ.get_or_init(|| Mutex::new(HashMap::new())) } // (WKAS liquidity, read ms)
static TWAPS: std::sync::OnceLock<Mutex<HashMap<String, (usize, u64, u64)>>> = std::sync::OnceLock::new();
fn twap_map() -> &'static Mutex<HashMap<String, (usize, u64, u64)>> { TWAPS.get_or_init(|| Mutex::new(HashMap::new())) } // (samples in window, MEASURED window span ms, read ms)
static AGE: std::sync::OnceLock<Mutex<HashMap<String, (u64, u64)>>> = std::sync::OnceLock::new();
fn age_map() -> &'static Mutex<HashMap<String, (u64, u64)>> { AGE.get_or_init(|| Mutex::new(HashMap::new())) } // (pool touch age s, read ms)
const MIN_MOVE10_USD: f64 = 250.0; // below this a KRC-20 feed is "thin": $250 of buying moves it 10%
const DIVERGE_BPS: f64 = 500.0;    // ≥2 venues quoting >5% apart → the published midpoint is a price NOBODY trades at
const TWAP_N: usize = 12;          // ~60s window at SLOW_EVERY=5s — kills single-block flash-loan spikes
/// USD trade size that moves a constant-product pool by `delta` (0.10 = 10%):
/// `dx = R_wkas·(√(1+δ)−1)/0.997`, priced through KAS/USD. The TOKEN reserve
/// cancels out of the closed form — only the WKAS side sets the cost — so
/// pool_read does not need to return it.
///
/// This is a TRADE SIZE, not the attacker's net cost: arbitrage fights back and
/// the move must survive the TWAP window. Never label it "cost to lie".
fn move_cost_usd(liq_wkas: f64, delta: f64, kas_usd: f64) -> f64 { liq_wkas * ((1.0 + delta).sqrt() - 1.0) / 0.997 * kas_usd }
/// JSON `f64|null` / `u64|null`. `null` means "we don't know" — which is NOT 0.
fn onum(v: Option<f64>) -> String { v.map_or("null".to_string(), |x| if x < 1.0 { format!("{x:.4}") } else { format!("{x:.2}") }) }
fn onum_u(v: Option<u64>) -> String { v.map_or("null".to_string(), |x| x.to_string()) }

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

/// Kraken REST backstop for the ONE real USD (ZUSD) quote in the whole book.
/// The WS leg stays primary — this only writes when Kraken's quote is already
/// >10s old, under the SAME source name (it refreshes a venue, it does not
/// invent one). Why it exists: `event_trigger=bbo` on a low-volume pair goes
/// quiet for tens of seconds, and once Kraken ages past STALE_MS, KAS/USD drops
/// to three USDT venues and the MAD filter silently switches OFF (it needs 4).
/// Why its OWN thread and not slow_thread: a full pool round takes ~30s with 65
/// pools, so a leg inside that loop cannot keep anything under 30s.
fn kraken_rest_thread(lp: Live) {
    loop {
        let a = agent();
        if let Some(p) = kraken_rest(&a, "KASUSD") { set_price_if_older(&lp, "KAS/USD", "Kraken", p, 10_000); }
        std::thread::sleep(Duration::from_secs(SLOW_EVERY));
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
    // per (pair, chain): the TWAP window as (WKAS price, read ms). The timestamp
    // is what lets a GAP re-arm the window — see the push below.
    let mut win: HashMap<String, Vec<(f64, u64)>> = HashMap::new();
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
            // one head-timestamp per chain per round — pool age is measured against
            // the CHAIN's clock, never ours (see chain_head_ts)
            let heads = chain_heads();
            // read pools in parallel (bounded concurrency) so the whole set refreshes in seconds, not a minute
            let pl = load_pools();
            for chunk in pl.chunks(12) {
                let reads: Vec<(String, &str, Option<(f64, f64, Option<u64>)>)> = std::thread::scope(|s| {
                    chunk.iter().map(|p| s.spawn(move || (format!("{}/USD", p.symbol), p.chain.as_str(), pool_read(&chain_rpcs(&p.chain), p))))
                        .collect::<Vec<_>>().into_iter().map(|h| h.join().unwrap()).collect()
                });
                for (pair, chain, res) in reads {
                    if let Some((px_kas, liq, bts)) = res {
                        // A read that prices at zero (WKAS side drained, token side not)
                        // is dropped by set_price and contributes NOTHING to the feed — so
                        // it must not contribute depth or age either. Writing it anyway let
                        // a drained pool win the `cheapest` selection and publish
                        // move_10pct_usd 0.0000 / thin:true on a feed whose real venue is
                        // deep. Same test as set_price, on purpose.
                        if px_kas <= 0.0 { continue; }
                        let key = format!("{pair}|{chain}");
                        let tms = now_ms();
                        let w = win.entry(key.clone()).or_default(); // window per (pair, chain)
                        // A GAP RE-ARMS THE WINDOW. Samples are only a TWAP while they are
                        // contiguous: after an RPC outage the surviving samples are minutes
                        // old, and keeping them would report a full, warm 12-sample window
                        // whose median is the pre-outage price — with halted:false, because
                        // the warm-up gate counts samples, not age.
                        if w.last().map_or(false, |(_, t)| tms.saturating_sub(*t) >= STALE_MS) { w.clear(); }
                        // TWAP the WKAS LEG ONLY, then multiply by the guarded KAS/USD.
                        // Windowing the product instead would price every KRC-20 token off
                        // a KAS quote up to 60s old for zero security: the KAS leg is
                        // defended by seven exchange venues, not by pool depth.
                        w.push((px_kas, tms)); if w.len() > TWAP_N { let d = w.len() - TWAP_N; w.drain(0..d); }
                        let samples = w.len();
                        // the MEASURED span of the window (oldest → newest sample), not the
                        // nominal samples × SLOW_EVERY: a pool round takes as long as the
                        // RPCs take, so the nominal figure is wrong in both directions.
                        let span_ms = tms.saturating_sub(w[0].1);
                        let med = median(&w.iter().map(|(p, _)| *p).collect::<Vec<_>>());
                        set_price(&lp, &pair, dex_source(chain), med * ku);
                        // CURRENT venue state, stamped — readers expire it (see liq_map)
                        liq_map().lock().unwrap().insert(key.clone(), (liq, tms));
                        twap_map().lock().unwrap().insert(key.clone(), (samples, span_ms, tms));
                        // "touch" age, not trade age: UniV2 _update also fires on mint/burn/sync
                        if let (Some(b), Some(head)) = (bts, heads.get(chain)) { age_map().lock().unwrap().insert(key, (head.saturating_sub(b), tms)); }
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
    /// 32-byte attestation record + per-node sigs (slash-observable)
    record_hex: String, record_sigs_json: String,
    ts: u64, round: u64,
}

/// Minimal LE script-number encoding of price_e8 (MESSAGE-FORMAT §8.2).
///
/// WITHDRAWN AS A SIGNING DOMAIN. We still PUBLISH these bytes (they are just a
/// re-encoding of the public price_e8, useful for building a script operand),
/// but the committee no longer signs blake2b of them: the preimage carries only
/// a bare integer — no pair, no expo, no round, no ts — so one feed's sigs
/// unlock any covenant on any other pair with a lower strike, and the feeds that
/// quantize to price_e8=0 carry valid sigs over blake2b(empty), which satisfies
/// every AtOrBelow gate forever.
///
/// TODO(cov/v2): the replacement is a single BOUND blob, signed as
/// blake2b256 of `"kaspulse/cov/v2" ‖ blake2b256(PAIR)[0..8] ‖ expo(i8) ‖
/// round_be(u64) ‖ ts_be(u64) ‖ mant_le`, emitted as `covenant.preimage` +
/// `covenant.signatures`. Sign mant+expo, NOT price_e8 — six live pairs lose
/// >1% to e8 quantization and three quantize to literally zero. The script
/// becomes `OpDup <0> <24> OpSubstr <baked_24B_prefix> OpEqualVerify` (OpSubstr
/// 0x7f is enabled at the pinned rusty-kaspa rev) to bind tag+pair+expo, then a
/// value compare on the mant tail, then the existing OpBlake2b + committee_tail.
/// No on-chain min_round floor is possible (script numbers are minimal-LE, so an
/// 8-byte BE round is not a numeric operand) — bind round/ts cryptographically
/// and enforce freshness with an nSequence/DAA relative timelock on the spend.
/// Ships with the v3 bump, AFTER re-proving it on TN10 — not before.
fn price_bytes(price_e8: i64) -> Vec<u8> {
    if price_e8 == 0 { return vec![]; }
    let neg = price_e8 < 0; let mut abs = price_e8.unsigned_abs(); let mut out = Vec::new();
    while abs > 0 { out.push((abs & 0xff) as u8); abs >>= 8; }
    if out.last().unwrap() & 0x80 != 0 { out.push(if neg { 0x80 } else { 0 }); } else if neg { *out.last_mut().unwrap() |= 0x80; }
    out
}
/// 32-byte bond attestation (v2):
/// slot(blake2b("kaspulse/bond/v2|{pair}")[0..8] ‖ round_be) ‖ mant_be ‖ expo_be.
///
/// expo is in the record because WITHOUT it mant=293800000@expo=-10 and
/// @expo=-9 produce byte-identical records — a 10x price move would be provably
/// unslashable. The pair-id is domain-separated so a v1 (24-byte) record can
/// never share a slot with a v2 one. Keep byte-identical to
/// `kaspulse_sdk::covenant::bond::attestation_record` — the script's OpSubstr
/// indices are derived from this layout.
fn attestation_record(pair: &str, round: u64, mant: u64, expo: i32) -> [u8; 32] {
    let h = blake2b_simd::Params::new().hash_length(32).hash(format!("kaspulse/bond/v2|{pair}").as_bytes());
    let mut r = [0u8; 32];
    r[..8].copy_from_slice(&h.as_bytes()[..8]);
    r[8..16].copy_from_slice(&round.to_be_bytes());
    r[16..24].copy_from_slice(&mant.to_be_bytes());
    r[24..32].copy_from_slice(&(expo as i64).to_be_bytes());
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
        r#"{{"threshold":{THRESHOLD},"num_nodes":{N_NODES},"signers":[{}],"message":"kaspulse/v2","covenant":"withdrawn — unbound preimage, see MESSAGE-FORMAT","updated_ts":{ts}}}"#,
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

    // ── hand the GUARDED KAS/USD to the KRC-20 multiplier (see ku_cell) and use
    //    the same number to price depth in dollars below ──
    let kas_row = rows.iter().find(|r| r.cfg.pair == "KAS/USD");
    let kas_px = kas_row.map(|r| r.med).unwrap_or(0.0);
    if kas_px > 0.0 { *ku_cell().lock().unwrap() = (kas_px, tms); }
    // …and the coupling runs the OTHER way too: while the KAS/USD breaker holds a
    // pre-move price, every KRC-20 price in this same envelope is that held number
    // × a pool ratio. Those feeds must carry the halt as well, or a 25% KAS drop
    // ships 58 freshly-signed `halted:false` prices that are 25% wrong. No KAS/USD
    // row at all (every venue stale) counts as halted for the same reason.
    let kas_halted = kas_row.map_or(true, |r| r.halted);

    // ── pass 2: sign the PUBLISHED price + render ──
    let mut objs = Vec::new();
    let mut per_pair: Vec<(String, String)> = Vec::new(); // "KAS-USD" -> FeedObj JSON
    let mut cat_rows: Vec<String> = Vec::new();           // /v1/feeds light catalog
    let mut feeds_live = 0usize;
    for r in &rows {
        let med = r.med;
        let price_e8 = (med * 1e8).round() as u64; // published as an integer convenience — NO LONGER a signing domain
        let (mant, expo) = mant_expo(med);
        // sign on CHANGE, re-sign unchanged prices only on the heartbeat
        let (msg, sigs_json, record_hex, record_sigs_json, signed_ts, signed_round) = match scache.get(&r.cfg.pair) {
            Some(c) if c.mant == mant && c.expo == expo && c.price_e8 == price_e8 && ts.saturating_sub(c.ts) < HEARTBEAT_S =>
                (c.msg.clone(), c.sigs_json.clone(), c.record_hex.clone(), c.record_sigs_json.clone(), c.ts, c.round),
            _ => {
                let msg = format!("kaspulse/v2|{}|{mant}|{expo}|{ts}|{round}", r.cfg.pair);
                let sigs_json = keys.iter().map(|k| format!("\"{}\"", sign(k, &msg))).collect::<Vec<_>>().join(",");
                // NOTE: no covenant-domain signature is produced here any more — see
                // price_bytes() for why the unbound preimage was withdrawn.
                let rec = attestation_record(&r.cfg.pair, round, mant, expo);
                let record_hex = hex::encode(rec);
                let record_sigs_json = keys.iter().map(|k| format!("\"{}\"", sign_bytes(k, &rec))).collect::<Vec<_>>().join(",");
                scache.insert(r.cfg.pair.clone(), SignCache {
                    mant, expo, price_e8, msg: msg.clone(), sigs_json: sigs_json.clone(),
                    record_hex: record_hex.clone(),
                    record_sigs_json: record_sigs_json.clone(), ts, round,
                });
                (msg, sigs_json, record_hex, record_sigs_json, ts, round)
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
        // ── per-venue state, FRESH reads only (a venue we haven't read inside
        //    STALE_MS doesn't get to vote on depth or age) ──
        let vst: Vec<(&str, f64, Option<u64>, usize, u64)> = { // (chain, liq_wkas, touch age s, TWAP samples, measured window ms)
            let (lm, am, tm) = (liq_map().lock().unwrap(), age_map().lock().unwrap(), twap_map().lock().unwrap());
            CHAINS.iter().filter_map(|c| {
                // only venues that ACTUALLY contributed to this price get to describe
                // its depth and age — otherwise "the winning venue" in the docs is a
                // pool whose read never reached the feed.
                if !r.srcs.iter().any(|(n, _, _)| n == dex_source(c)) { return None; }
                let k = format!("{}|{c}", r.cfg.pair);
                let (l, t) = *lm.get(&k)?;
                if tms.saturating_sub(t) >= STALE_MS { return None; }
                let fresh = |t: &u64| tms.saturating_sub(*t) < STALE_MS;
                let (smp, span) = tm.get(&k).filter(|(_, _, t)| fresh(t)).map_or((0usize, 0u64), |(s, sp, _)| (*s, *sp));
                Some((*c, l, am.get(&k).filter(|(_, t)| fresh(t)).map(|(a, _)| *a), smp, span))
            }).collect()
        };
        // An attacker buys the SHALLOWEST contributing pool, so that is the venue
        // that sets this feed's depth — the old max-fold was backwards.
        let cheapest = vst.iter().copied().min_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let liq = vst.iter().map(|(_, l, _, _, _)| *l).fold(0.0_f64, f64::max); // legacy scalar: the DEEPEST venue
        let cost = |d: f64| if kas_px > 0.0 { cheapest.map(|(_, l, _, _, _)| move_cost_usd(l, d, kas_px)) } else { None };
        let (move10, depth2) = (cost(0.10), cost(0.02));
        let pool_age = cheapest.and_then(|(_, _, a, _, _)| a); // age of the venue that WON the depth selection
        // twap_samples / twap_window_s = the LEAST-warmed contributing venue: the
        // published median is only as TWAPed as its youngest window.
        let (tsamp, tspan) = if r.cfg.kind == "krc20" {
            vst.iter().map(|(_, _, _, s, sp)| (*s, *sp)).min().unwrap_or((0, 0))
        } else { (0, 0) };
        // `thin` is now a DOLLAR test. Unknown depth on a KRC-20 feed counts as
        // thin — we can't demonstrate it isn't.
        let thin = r.cfg.kind == "krc20" && move10.map_or(true, |m| m < MIN_MOVE10_USD);
        let divergent = r.srcs.len() >= 2 && spread > DIVERGE_BPS;
        // COLD START: on restart the TWAP window and the breaker state are both
        // empty, so the first observation becomes the breaker anchor unchecked and
        // spot would be published as a "TWAP". Hold the feed until the window is
        // half full — 6 pool rounds, so as long as 6 rounds take (a pool round is
        // SLOW_EVERY plus however long 65 reads take, not SLOW_EVERY). It must set
        // HALTED, not degraded: sdk Feed::verify
        // rejects on halted and peg_ok only — `degraded` is parsed and never
        // consulted, so gating on it would stop exactly zero consumers.
        // A KRC-20 feed also inherits the KAS/USD halt: its price IS a KAS price.
        let halted_out = r.halted || (r.cfg.kind == "krc20" && (tsamp < TWAP_N / 2 || kas_halted));
        let venues_j = if r.cfg.kind == "krc20" && !vst.is_empty() {
            let items: Vec<String> = vst.iter().map(|(c, l, _, _, _)| format!(
                r#"{{"chain":"{c}","liq_wkas":{l:.1},"move_10pct_usd":{}}}"#,
                onum(if kas_px > 0.0 { Some(move_cost_usd(*l, 0.10, kas_px)) } else { None }))).collect();
            format!(r#","venues":[{}]"#, items.join(","))
        } else { String::new() };
        let src_j: Vec<String> = r.srcs.iter().map(|(n, p, a)| format!(r#"{{"name":"{n}","price":{},"age_ms":{a}}}"#, enum_(*p))).collect();
        let out_j: Vec<String> = r.outliers.iter().map(|n| format!("\"{n}\"")).collect();
        // Igra venues are named Igra-Zealous / Igra-KaspaCom (not the old "Igra-DEX")
        let peg_field = if r.srcs.iter().any(|(n, _, _)| is_igra_source(n)) {
            match igra_peg_ok { Some(ok) => format!(r#","peg_ok":{ok}"#), None => r#","peg_ok":null"#.to_string() }
        } else { String::new() };
        // `signatures` is GONE: the committee no longer signs blake2b(price_bytes).
        // price_e8/price_bytes stay because they are just the public price in another
        // encoding; `record` + `record_signatures` are the bond domain, which IS bound
        // (pair ‖ round ‖ mant ‖ expo) and unaffected by the withdrawal.
        let cov_j = format!(
            r#"{{"price_e8":{price_e8},"price_bytes":"{}","record":"{record_hex}","record_signatures":[{record_sigs_json}]}}"#,
            hex::encode(price_bytes(price_e8 as i64)));
        let h = hist.entry(r.cfg.pair.clone()).or_default(); h.push((ts, med)); if h.len() > HISTORY { let d = h.len() - HISTORY; h.drain(0..d); }
        let hist_j: Vec<String> = h.iter().map(|(t, p)| format!("[{t},{}]", enum_(*p))).collect();
        // twap_window_s is MEASURED (oldest → newest sample in the winning venue's
        // window), not the nominal tsamp × SLOW_EVERY: a pool round takes as long as
        // the RPCs take, so the nominal figure was wrong in both directions and the
        // published price could be older than the window it advertised.
        // NOTE: twap*/move_10pct_usd/depth_2pct_usd/pool_age_s/venues/divergent are
        // UNSIGNED ADVISORY METADATA. The v2 signed message is frozen (pair|mant|
        // expo|ts|round) and Feed::verify demands field equality, so none of them
        // can be added to it — they describe the price, they don't attest to it.
        let obj = format!(
            r#"{{"pair":"{}","kind":"{}","price":{},"price_e8":{price_e8},"mant":{mant},"expo":{expo},"sources":[{}],"num_sources":{},"outliers":[{}],"divergent":{divergent},"halted":{},"degraded":{}{peg_field},"freshest_ms":{freshest},"low":{},"high":{},"spread_bps":{:.2},"median":{},"twap":{},"twap_samples":{tsamp},"twap_window_s":{},"liq_wkas":{:.0},"move_10pct_usd":{},"depth_2pct_usd":{},"pool_age_s":{},"thin":{thin}{venues_j},"signers":[{signers_j}],"threshold":{threshold_eff},"signatures":[{sigs_j}],"message":"{msg}","signed_ts":{signed_ts},"signed_round":{signed_round},"covenant":{cov_j},"history":[{}]}}"#,
            r.cfg.pair, r.cfg.kind, enum_(med), src_j.join(","), r.srcs.len(), out_j.join(","), halted_out, r.degraded, enum_(lo), enum_(hi), spread, enum_(med),
            r.cfg.kind == "krc20" && tsamp >= TWAP_N, (tspan + 500) / 1000, liq, onum(move10), onum(depth2), onum_u(pool_age), hist_j.join(",")
        );
        // per-pair map (dash form, uppercase) — /v1/feed/{PAIR} serves this
        // string directly instead of re-parsing the whole envelope per request
        per_pair.push((r.cfg.pair.replace('/', "-").to_uppercase(), obj.clone()));
        // light catalog row — what dashboards poll instead of the full envelope
        cat_rows.push(format!(
            r#"{{"pair":"{}","kind":"{}","price":{},"num_sources":{},"halted":{},"degraded":{},"thin":{thin},"liq_wkas":{:.0},"move_10pct_usd":{},"pool_age_s":{},"spread_bps":{:.2},"freshest_ms":{freshest}}}"#,
            r.cfg.pair, r.cfg.kind, enum_(med), r.srcs.len(), halted_out, r.degraded, liq, onum(move10), onum_u(pool_age), spread));
        if !halted_out && !r.srcs.is_empty() { feeds_live += 1; }
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
    println!("  majors: Kraken+Bybit+OKX+Coinbase (WS, sub-second) + KuCoin/Gate/MEXC (REST {SLOW_EVERY}s); OKX+Coinbase are BTC/ETH only — neither lists KAS");
    println!("  KRC-20: direct Kasplex/Igra DEX pool reads (cross-checked RPCs)");

    let remote: aggregate::RemoteBook = Arc::new(Mutex::new(HashMap::new()));
    aggregate::spawn(remote.clone());

    std::thread::spawn(discover_thread);
    let lp: Live = Arc::new(Mutex::new(HashMap::new()));
    for (f, lpc) in [ws_kraken as fn(Live), ws_bybit, ws_okx, ws_coinbase, slow_thread, kraken_rest_thread].into_iter().zip(std::iter::repeat(lp.clone())) {
        std::thread::spawn(move || f(lpc));
    }

    let state = Arc::new(http::PubState::new());
    {
        let (state, lp, remote) = (state.clone(), lp.clone(), remote.clone());
        std::thread::spawn(move || {
            // `round` is a WALL-CLOCK SLOT, never a per-process counter. A counter
            // restarts at 1, so every restart re-signs slots 1..N at DIFFERENT prices
            // under the SAME keys — genuine, script-verifiable equivocation proofs
            // against our own bond, free to anyone who archived /v1/feed across a
            // deploy. MILLISECOND resolution is load-bearing: at SERVE_MS=400 a
            // seconds-based slot would hold 2-3 different mantissas per slot and
            // manufacture equivocation CONTINUOUSLY — strictly worse than the counter.
            let mut hwm = read_round_hwm();
            let mut reserved = hwm; // the on-disk reservation we already own
            let mut hist = HashMap::new(); let mut scache = HashMap::new(); let mut bstate = HashMap::new();
            loop {
                let slot = now_ms() / SERVE_MS;
                // only reachable if the clock moved backwards (NTP step, VM restore);
                // we keep issuing hwm+1 so the record stream stays injective
                if slot <= hwm { eprintln!("KASPULSE_ROUND_REGRESSION slot={slot} hwm={hwm} — clock went backwards; issuing hwm+1"); }
                let round = slot.max(hwm + 1);
                hwm = round;
                // reserve BEFORE signing — see read_round_hwm. Once per lease, so
                // this is one ~10-byte write a minute, not one per round.
                if round >= reserved { reserved = round + ROUND_HWM_LEASE; write_round_hwm(reserved); }
                let b = build(&lp, &keys, round, &mut hist, &mut scache, &mut bstate, &remote);
                state.publish(b.envelope, b.per_pair, b.catalog, b.committee, round, load_pools().len(), b.feeds_total, b.feeds_live);
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
        let r = attestation_record("KAS/USD", 42, 2_900_000, -10);
        let h = blake2b_simd::Params::new().hash_length(32).hash(b"kaspulse/bond/v2|KAS/USD");
        assert_eq!(&r[..8], &h.as_bytes()[..8]);
        assert_eq!(&r[8..16], &42u64.to_be_bytes());
        assert_eq!(&r[16..24], &2_900_000u64.to_be_bytes());
        assert_eq!(&r[24..32], &(-10i64).to_be_bytes());
        // the v1 domain (bare pair) must not collide with v2
        let h1 = blake2b_simd::Params::new().hash_length(32).hash(b"KAS/USD");
        assert_ne!(&r[..8], &h1.as_bytes()[..8]);
        // the worked example in docs/MESSAGE-FORMAT.md §8.1 — pinned here so the
        // spec and the code can never drift again (they did: §8.1 documented the
        // v1 24-byte layout for a week after the code moved to 32)
        assert_eq!(hex::encode(attestation_record("KAS/USD", 4242, 824_000_000, -10)),
            "9dab6487a796ddcb000000000000109200000000311d3e00fffffffffffffff6");
        // expo is IN the slot payload: a 10x move is now slashable
        let a = attestation_record("KAS/USD", 42, 293_800_000, -10);
        let b = attestation_record("KAS/USD", 42, 293_800_000, -9);
        assert_eq!(a[..16], b[..16]);   // same slot
        assert_ne!(a[16..], b[16..]);   // different price → equivocation
    }

    #[test]
    fn round_slot_is_monotonic_across_restart() {
        // the shape of the loop in main(): a wall-clock slot, floored by the
        // persisted high-water mark. Two "processes" over the same clock must
        // never issue the same round twice.
        let step = |slot: u64, hwm: &mut u64| { let r = slot.max(*hwm + 1); *hwm = r; r };
        let t0 = 1_753_000_000_000u64;
        let mut hwm = 0u64;
        let a: Vec<u64> = (0..5).map(|i| step((t0 + i * SERVE_MS) / SERVE_MS, &mut hwm)).collect();
        // restart: hwm reloaded from disk, clock has moved on
        let mut hwm2 = a[2]; // a stale on-disk hwm (persisted before the crash)
        let b: Vec<u64> = (10..15).map(|i| step((t0 + i * SERVE_MS) / SERVE_MS, &mut hwm2)).collect();
        assert!(a.windows(2).all(|w| w[1] > w[0]));
        assert!(b.windows(2).all(|w| w[1] > w[0]));
        assert!(b[0] > a[4], "a restart must never replay a slot");
        // clock jumps backwards a full day → hwm floor still forces a fresh slot
        let mut hwm3 = *b.last().unwrap();
        assert_eq!(step((t0 - 86_400_000) / SERVE_MS, &mut hwm3), b[4] + 1);
    }

    #[test]
    fn round_hwm_is_a_reservation_never_a_log() {
        // the on-disk value must always be ≥ every round ever issued, so that a
        // crash between writes can only make the next process SKIP slots, never
        // replay them. Model of the loop in main().
        let mut reserved = 0u64;
        let mut disk = 0u64;
        let mut hwm = 0u64;
        let mut issued = Vec::new();
        let t0 = 1_753_000_000_000u64;
        for i in 0..400u64 {
            let round = (t0 + i * SERVE_MS) / SERVE_MS;
            let round = round.max(hwm + 1);
            hwm = round;
            if round >= reserved { reserved = round + ROUND_HWM_LEASE; disk = reserved; }
            issued.push(round);
            // at EVERY point mid-run, the disk value is an upper bound
            assert!(disk >= round, "round {round} was issued above the on-disk floor {disk}");
        }
        // crash here: the next process reloads `disk` and must not reissue anything
        let mut hwm2 = disk;
        let next = ((t0 + 401 * SERVE_MS) / SERVE_MS).max(hwm2 + 1);
        hwm2 = next;
        assert!(next > *issued.last().unwrap(), "a restart must never replay a slot");
        let _ = hwm2;
    }

    #[test]
    fn krc20_inherits_the_kas_usd_halt() {
        // a KRC-20 price IS a KAS price × a pool ratio, so while the KAS/USD breaker
        // holds a pre-move number the derived feeds are wrong by the same amount.
        let out = |kind: &str, row_halted: bool, tsamp: usize, kas_halted: bool|
            row_halted || (kind == "krc20" && (tsamp < TWAP_N / 2 || kas_halted));
        assert!(out("krc20", false, TWAP_N, true));   // warm window, halted denominator
        assert!(!out("krc20", false, TWAP_N, false)); // warm window, healthy denominator
        assert!(!out("major", false, 0, true));       // KAS/USD's own halt is r.halted
        // "no KAS/USD row at all" is passed in as kas_halted=true by build()
        assert!(out("krc20", false, TWAP_N, true));
    }

    #[test]
    fn twap_window_re_arms_after_a_gap() {
        // model of the push in slow_thread: a gap ≥ STALE_MS clears the window, so
        // tsamp counts only CONTIGUOUS samples and the warm-up halt fires again.
        let push = |w: &mut Vec<(f64, u64)>, px: f64, tms: u64| {
            if w.last().map_or(false, |(_, t)| tms.saturating_sub(*t) >= STALE_MS) { w.clear(); }
            w.push((px, tms));
            if w.len() > TWAP_N { let d = w.len() - TWAP_N; w.drain(0..d); }
        };
        let mut w = Vec::new();
        for i in 0..TWAP_N as u64 { push(&mut w, 1.0, i * 5_000); }
        assert_eq!(w.len(), TWAP_N); // fully warm
        // 10-minute outage, then the price has doubled
        let after = (TWAP_N as u64 - 1) * 5_000 + 600_000;
        push(&mut w, 2.0, after);
        assert_eq!(w.len(), 1, "stale samples must not stay in the window");
        assert!(w.len() < TWAP_N / 2, "the warm-up halt must re-arm");
        // and the measured span is the real one, not a nominal 60s
        assert_eq!(after.saturating_sub(w[0].1), 0);
    }

    #[test]
    fn move_cost_matches_the_closed_form() {
        // dx = R·(√1.1−1)/0.997 · kas_usd — the token reserve does NOT appear
        let r = 1000.0; let kas = 0.029365;
        let want = r * (1.1f64.sqrt() - 1.0) / 0.997 * kas;
        assert!((move_cost_usd(r, 0.10, kas) - want).abs() < 1e-12);
        // the old MIN_LIQ_WKAS=1000 line was $1.44 of buying — the reason `thin`
        // became a dollar test
        assert!(move_cost_usd(1000.0, 0.10, kas) < 2.0);
        // a 2% move is always cheaper than a 10% one, and both scale linearly in R
        assert!(move_cost_usd(r, 0.02, kas) < move_cost_usd(r, 0.10, kas));
        assert!((move_cost_usd(2.0 * r, 0.10, kas) - 2.0 * move_cost_usd(r, 0.10, kas)).abs() < 1e-9);
        // MIN_MOVE10_USD is a real gate: at $0.029 it takes ~174k WKAS (~$5.1k of
        // one-sided depth) to stop being "thin"
        assert!(move_cost_usd(170_000.0, 0.10, kas) < MIN_MOVE10_USD);
        assert!(move_cost_usd(175_000.0, 0.10, kas) > MIN_MOVE10_USD);
    }

    #[test]
    fn warmup_gate_halts_krc20_until_the_window_fills() {
        let gate = |kind: &str, tsamp: usize| kind == "krc20" && tsamp < TWAP_N / 2;
        let twap = |kind: &str, tsamp: usize| kind == "krc20" && tsamp >= TWAP_N;
        assert!(gate("krc20", 0));            // cold start: breaker anchor is unchecked
        assert!(gate("krc20", TWAP_N / 2 - 1));
        assert!(!gate("krc20", TWAP_N / 2));  // half a window
        assert!(!gate("major", 0));           // majors never enter the window at all
        // and the twap flag stays FALSE until the window is genuinely full
        assert!(!twap("krc20", TWAP_N - 1));
        assert!(twap("krc20", TWAP_N));
        assert!(!twap("major", TWAP_N));
    }

    #[test]
    fn null_is_not_zero() {
        assert_eq!(onum(None), "null");
        assert_eq!(onum_u(None), "null");
        assert_eq!(onum(Some(0.0)), "0.0000"); // sub-$1 depth keeps 4 decimals
        assert_eq!(onum(Some(1234.5)), "1234.50");
        assert_eq!(onum_u(Some(303 * 86_400)), "26179200");
    }

    #[test]
    fn eth_call_cross_quorum_requires_two_when_configured() {
        // unreachable endpoints → empty got → None (no single-response accept with 2 configured)
        let rpcs = vec!["http://127.0.0.1:1".into(), "http://127.0.0.1:2".into()];
        assert!(eth_call_cross(&rpcs, "0xabc", "0x00").is_none());
    }
}

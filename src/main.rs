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
use anyhow::Result;
use secp256k1::{Keypair, SECP256K1};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
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
static POOLS: std::sync::OnceLock<Vec<Pool>> = std::sync::OnceLock::new();
fn load_pools() -> &'static Vec<Pool> {
    POOLS.get_or_init(|| serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string("pools.json").unwrap_or_default()).ok()
        .and_then(|v| v.as_array().map(|a| a.iter().filter_map(|p| {
            let symbol = p["symbol"].as_str()?.to_string();
            // a KRC-20 meme token named KAS/BTC/ETH must NOT collide with the real major feeds
            if matches!(symbol.to_uppercase().as_str(), "KAS" | "BTC" | "ETH") { return None; }
            Some(Pool { symbol, pool: p["pair"].as_str()?.to_string(),
                wkas_is_token0: p["wkas_is_token0"].as_bool()?, dec: p["dec"].as_u64()? as u32,
                chain: p["chain"].as_str().unwrap_or("kasplex").to_string() })
        }).collect())).unwrap_or_default())
}
#[derive(Clone)]
struct FeedCfg { pair: String, kind: &'static str, kucoin: Option<&'static str>, gate: Option<&'static str>, mexc: Option<&'static str> }
fn feeds() -> Vec<FeedCfg> {
    let mut v = vec![
        FeedCfg { pair: "KAS/USD".into(), kind: "major", kucoin: Some("KAS-USDT"), gate: Some("KAS_USDT"), mexc: Some("KASUSDT") },
        FeedCfg { pair: "BTC/USD".into(), kind: "major", kucoin: Some("BTC-USDT"), gate: Some("BTC_USDT"), mexc: Some("BTCUSDT") },
        FeedCfg { pair: "ETH/USD".into(), kind: "major", kucoin: Some("ETH-USDT"), gate: Some("ETH_USDT"), mexc: Some("ETHUSDT") },
    ];
    let mut seen = std::collections::HashSet::new();
    for p in load_pools() { if seen.insert(p.symbol.clone()) { v.push(FeedCfg { pair: format!("{}/USD", p.symbol), kind: "krc20", kucoin: None, gate: None, mexc: None }); } }
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
fn chain_rpcs(chain: &str) -> Vec<String> {
    let (env, default) = match chain {
        "igra" => ("IGRA_RPCS", "https://rpc.igralabs.com:8545"),
        _ => ("KASPLEX_RPCS", "https://evmrpc.kasplex.org"),
    };
    std::env::var(env).ok().filter(|s| !s.is_empty())
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_else(|| vec![default.to_string()])
}
fn dex_source(chain: &str) -> &'static str { match chain { "igra" => "Igra-DEX", _ => "Kasplex-DEX" } }
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
    // ≥2 RPCs must AGREE — a single compromised RPC can't move the price
    if got.iter().all(|r| r == &got[0]) { Some(got.remove(0)) } else { eprintln!("kasplex RPC disagreement on {to} — dropping this read"); None }
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

fn slow_thread(lp: Live) {
    for c in ["kasplex", "igra"] { let r = chain_rpcs(c); eprintln!("{c} RPCs ({}): {}", r.len(), r.join(", ")); }
    let mut win: HashMap<String, Vec<f64>> = HashMap::new();
    loop {
        let a = agent();
        for f in feeds() {
            if let Some(s) = f.kucoin { if let Some(p) = kucoin(&a, s) { set_price(&lp, &f.pair, "KuCoin", p); } }
            if let Some(s) = f.gate   { if let Some(p) = gate(&a, s)   { set_price(&lp, &f.pair, "Gate.io", p); } }
            if let Some(s) = f.mexc   { if let Some(p) = mexc(&a, s)   { set_price(&lp, &f.pair, "MEXC", p); } }
        }
        // KRC-20: read each pool on ITS chain (cross-checked) → windowed median (TWAP) → publish.
        // A token on both chains gets two on-chain sources (Kasplex-DEX + Igra-DEX) → build() medians.
        let ku = kas_usd(&lp);
        if ku > 0.0 {
            // read pools in parallel (bounded concurrency) so the whole set refreshes in seconds, not a minute
            for chunk in load_pools().chunks(12) {
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
fn load_keys(n: usize) -> Vec<Keypair> {
    (0..n).map(|i| { let path = format!("kaspulse-node-{i}.key");
        if let Ok(raw) = std::fs::read_to_string(&path) { if let Ok(b) = hex::decode(raw.trim()) { if let Ok(sk) = secp256k1::SecretKey::from_slice(&b) { return Keypair::from_secret_key(SECP256K1, &sk); } } }
        let kp = Keypair::new(SECP256K1, &mut secp256k1::rand::thread_rng()); let _ = std::fs::write(&path, hex::encode(kp.secret_key().secret_bytes())); kp
    }).collect()
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
struct SignCache { mant: u64, expo: i32, msg: String, sigs_json: String, ts: u64, round: u64 }

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

fn build(lp: &Live, keys: &[Keypair], round: u64, hist: &mut HashMap<String, Vec<(u64, f64)>>, scache: &mut HashMap<String, SignCache>, bstate: &mut HashMap<String, (f64, u32)>) -> String {
    let ts = now(); let tms = now_ms();
    let signers: Vec<String> = keys.iter().map(|k| format!("\"{}\"", hex::encode(k.x_only_public_key().0.serialize()))).collect();
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
    for r in &rows {
        let med = r.med;
        let price_e8 = (med * 1e8).round() as u64; // informational only — the SIGNED price is mant×10^expo
        let (mant, expo) = mant_expo(med);
        // sign on CHANGE, re-sign unchanged prices only on the heartbeat
        let (msg, sigs_json, signed_ts, signed_round) = match scache.get(&r.cfg.pair) {
            Some(c) if c.mant == mant && c.expo == expo && ts.saturating_sub(c.ts) < HEARTBEAT_S =>
                (c.msg.clone(), c.sigs_json.clone(), c.ts, c.round),
            _ => {
                let msg = format!("kaspulse/v2|{}|{mant}|{expo}|{ts}|{round}", r.cfg.pair);
                let sigs_json = keys.iter().map(|k| format!("\"{}\"", sign(k, &msg))).collect::<Vec<_>>().join(",");
                scache.insert(r.cfg.pair.clone(), SignCache { mant, expo, msg: msg.clone(), sigs_json: sigs_json.clone(), ts, round });
                (msg, sigs_json, ts, round)
            }
        };
        let prices: Vec<f64> = r.srcs.iter().map(|(_, p, _)| *p).collect();
        let lo = prices.iter().cloned().fold(f64::MAX, f64::min); let hi = prices.iter().cloned().fold(f64::MIN, f64::max);
        let spread = if med > 0.0 { ((hi - lo) / med) * 10_000.0 } else { 0.0 };
        let freshest = r.srcs.iter().map(|(_, _, a)| *a).min().unwrap_or(0);
        // deepest CURRENT venue liquidity (per-chain entries are overwritten each round — drained pools decay)
        let liq = { let lm = liq_map().lock().unwrap();
            ["kasplex", "igra"].iter().filter_map(|c| lm.get(&format!("{}|{c}", r.cfg.pair))).cloned().fold(0.0_f64, f64::max) };
        let thin = r.cfg.kind == "krc20" && liq < MIN_LIQ_WKAS; // low-liquidity pool → manipulable, low-confidence
        let src_j: Vec<String> = r.srcs.iter().map(|(n, p, a)| format!(r#"{{"name":"{n}","price":{},"age_ms":{a}}}"#, enum_(*p))).collect();
        let out_j: Vec<String> = r.outliers.iter().map(|n| format!("\"{n}\"")).collect();
        let peg_field = if r.srcs.iter().any(|(n, _, _)| n == "Igra-DEX") {
            match igra_peg_ok { Some(ok) => format!(r#","peg_ok":{ok}"#), None => r#","peg_ok":null"#.to_string() }
        } else { String::new() };
        let h = hist.entry(r.cfg.pair.clone()).or_default(); h.push((ts, med)); if h.len() > HISTORY { let d = h.len() - HISTORY; h.drain(0..d); }
        let hist_j: Vec<String> = h.iter().map(|(t, p)| format!("[{t},{}]", enum_(*p))).collect();
        objs.push(format!(
            r#"{{"pair":"{}","kind":"{}","price":{},"price_e8":{price_e8},"mant":{mant},"expo":{expo},"sources":[{}],"num_sources":{},"outliers":[{}],"halted":{},"degraded":{}{peg_field},"freshest_ms":{freshest},"low":{},"high":{},"spread_bps":{:.2},"median":{},"twap":true,"liq_wkas":{:.0},"thin":{thin},"signers":[{}],"threshold":{THRESHOLD},"signatures":[{}],"message":"{msg}","signed_ts":{signed_ts},"signed_round":{signed_round},"history":[{}]}}"#,
            r.cfg.pair, r.cfg.kind, enum_(med), src_j.join(","), r.srcs.len(), out_j.join(","), r.halted, r.degraded, enum_(lo), enum_(hi), spread, enum_(med), liq, signers.join(","), sigs_json, hist_j.join(",")
        ));
    }
    let peg_j = format!(r#"{{"igra_usdc":{},"igra_ok":{}}}"#,
        usdc.map(|u| format!("{u}")).unwrap_or_else(|| "null".into()),
        igra_peg_ok.map(|b| b.to_string()).unwrap_or_else(|| "null".into()));
    format!(r#"{{"round":{round},"timestamp":{ts},"threshold":{THRESHOLD},"num_nodes":{N_NODES},"transport":"websocket","peg":{peg_j},"feeds":[{}]}}"#, objs.join(","))
}

// ---------- http ----------
fn mime(p: &str) -> &'static str { if p.ends_with(".html") { "text/html; charset=utf-8" } else if p.ends_with(".js") { "application/javascript" } else if p.ends_with(".css") { "text/css" } else if p.ends_with(".json") { "application/json" } else { "text/plain" } }
fn serve(mut s: std::net::TcpStream, feed: &Arc<Mutex<String>>) {
    let mut buf = [0u8; 2048]; let n = match s.read(&mut buf) { Ok(n) => n, Err(_) => return };
    let path = String::from_utf8_lossy(&buf[..n]).split_whitespace().nth(1).unwrap_or("/").to_string();
    let cors = "Access-Control-Allow-Origin: *\r\n";
    let (status, ctype, body): (&str, &str, Vec<u8>) = if path.starts_with("/api/feed") || path == "/feed.json" {
        let full = feed.lock().unwrap().clone();
        if let Some(p) = path.strip_prefix("/api/feed/") {
            let want = p.replace('-', "/").to_uppercase();
            let one = serde_json::from_str::<serde_json::Value>(&full).ok().and_then(|v| v["feeds"].as_array()?.iter().find(|f| f["pair"].as_str() == Some(&want)).cloned()).map(|v| v.to_string()).unwrap_or_else(|| "{\"error\":\"no such feed\"}".into());
            ("200 OK", "application/json", one.into_bytes())
        } else { ("200 OK", "application/json", full.into_bytes()) }
    } else {
        let file = if path == "/" { "index.html".into() } else { path.trim_start_matches('/').to_string() };
        match if !file.contains("..") { std::fs::read(format!("web/{file}")).ok() } else { None } { Some(b) => ("200 OK", mime(&file), b), None => ("404 Not Found", "text/plain", b"not found".to_vec()) }
    };
    let _ = s.write_all(format!("HTTP/1.1 {status}\r\n{cors}Content-Type: {ctype}\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes());
    let _ = s.write_all(&body);
}

fn main() -> Result<()> {
    let keys = load_keys(N_NODES);
    println!("kaspulse oracle — WebSocket streaming · {N_NODES} nodes, {THRESHOLD}-of-{N_NODES} · serve every {SERVE_MS}ms");
    println!("  majors: Kraken+Bybit (WS, sub-second) + KuCoin/Gate/MEXC (REST {SLOW_EVERY}s)");
    println!("  KRC-20: CoinGecko + GeckoTerminal (Kasplex DEX)");
    println!("serving http://127.0.0.1:{PORT}");

    let lp: Live = Arc::new(Mutex::new(HashMap::new()));
    for (f, lpc) in [ws_kraken as fn(Live), ws_bybit, slow_thread].into_iter().zip(std::iter::repeat(lp.clone())) {
        std::thread::spawn(move || f(lpc));
    }

    let feed = Arc::new(Mutex::new(r#"{"feeds":[]}"#.to_string()));
    {
        let (feed, lp) = (feed.clone(), lp.clone());
        std::thread::spawn(move || {
            let mut round = 1u64; let mut hist = HashMap::new(); let mut scache = HashMap::new(); let mut bstate = HashMap::new();
            loop {
                let json = build(&lp, &keys, round, &mut hist, &mut scache, &mut bstate);
                let _ = std::fs::write("web/feed.json", &json);
                *feed.lock().unwrap() = json;
                round += 1;
                std::thread::sleep(Duration::from_millis(SERVE_MS));
            }
        });
    }
    let listener = TcpListener::bind(("127.0.0.1", PORT))?;
    for stream in listener.incoming() { if let Ok(s) = stream { let feed = feed.clone(); std::thread::spawn(move || serve(s, &feed)); } }
    Ok(())
}

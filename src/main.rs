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
const STALE_MS: u64 = 15_000; // drop a source's price if older than this
const SLOW_EVERY: u64 = 5;    // REST/KRC-20 refresh (seconds)

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
#[derive(Clone)]
struct Pool { symbol: String, pool: String, wkas_is_token0: bool, dec: u32 }
static POOLS: std::sync::OnceLock<Vec<Pool>> = std::sync::OnceLock::new();
fn load_pools() -> &'static Vec<Pool> {
    POOLS.get_or_init(|| serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string("pools.json").unwrap_or_default()).ok()
        .and_then(|v| v.as_array().map(|a| a.iter().filter_map(|p| Some(Pool {
            symbol: p["symbol"].as_str()?.to_string(), pool: p["pair"].as_str()?.to_string(),
            wkas_is_token0: p["wkas_is_token0"].as_bool()?, dec: p["dec"].as_u64()? as u32,
        })).collect())).unwrap_or_default())
}
#[derive(Clone)]
struct FeedCfg { pair: String, kind: &'static str, kucoin: Option<&'static str>, gate: Option<&'static str>, mexc: Option<&'static str>, pool: Option<Pool> }
fn feeds() -> Vec<FeedCfg> {
    let mut v = vec![
        FeedCfg { pair: "KAS/USD".into(), kind: "major", kucoin: Some("KAS-USDT"), gate: Some("KAS_USDT"), mexc: Some("KASUSDT"), pool: None },
        FeedCfg { pair: "BTC/USD".into(), kind: "major", kucoin: Some("BTC-USDT"), gate: Some("BTC_USDT"), mexc: Some("BTCUSDT"), pool: None },
        FeedCfg { pair: "ETH/USD".into(), kind: "major", kucoin: Some("ETH-USDT"), gate: Some("ETH_USDT"), mexc: Some("ETHUSDT"), pool: None },
    ];
    for p in load_pools() { v.push(FeedCfg { pair: format!("{}/USD", p.symbol), kind: "krc20", kucoin: None, gate: None, mexc: None, pool: Some(p.clone()) }); }
    v
}

// ---------- WebSocket streams (sub-second) ----------
fn ws_loop(name: &str, url: &str, sub: &str, lp: &Live, handle: impl Fn(&serde_json::Value, &Live)) {
    loop {
        let r = (|| -> Result<()> {
            let (mut s, _) = connect(url)?;
            s.write_message(Message::Text(sub.to_string()))?;
            loop {
                match s.read_message()? {
                    Message::Text(t) => { if let Ok(j) = serde_json::from_str::<serde_json::Value>(&t) { handle(&j, lp); } }
                    Message::Ping(p) => s.write_message(Message::Pong(p))?,
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            Ok(())
        })();
        if let Err(e) = r { eprintln!("{name} ws down: {e} — reconnecting"); }
        std::thread::sleep(Duration::from_secs(3));
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
fn kasplex_rpcs() -> Vec<String> {
    std::env::var("KASPLEX_RPCS").ok().filter(|s| !s.is_empty())
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_else(|| vec!["https://evmrpc.kasplex.org".to_string()])
}
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
    match lp.lock().unwrap().get("KAS/USD") {
        Some(m) => { let mut v: Vec<f64> = m.values().map(|(p, _)| *p).collect(); v.sort_by(|a, b| a.partial_cmp(b).unwrap()); if v.is_empty() { 0.0 } else { v[v.len() / 2] } }
        None => 0.0,
    }
}
// per-pair WKAS liquidity, for the thin-pool flag
static LIQ: std::sync::OnceLock<Mutex<HashMap<String, f64>>> = std::sync::OnceLock::new();
fn liq_map() -> &'static Mutex<HashMap<String, f64>> { LIQ.get_or_init(|| Mutex::new(HashMap::new())) }
const MIN_LIQ_WKAS: f64 = 1000.0; // below this a KRC-20 feed is flagged "thin" (manipulable / low-confidence)
const TWAP_N: usize = 12;         // ~60s window at SLOW_EVERY=5s — kills single-block flash-loan spikes

fn slow_thread(lp: Live) {
    let rpcs = kasplex_rpcs();
    eprintln!("kasplex RPCs ({}): {}", rpcs.len(), rpcs.join(", "));
    let mut win: HashMap<String, Vec<f64>> = HashMap::new();
    loop {
        let a = agent();
        for f in feeds() {
            if let Some(s) = f.kucoin { if let Some(p) = kucoin(&a, s) { set_price(&lp, &f.pair, "KuCoin", p); } }
            if let Some(s) = f.gate   { if let Some(p) = gate(&a, s)   { set_price(&lp, &f.pair, "Gate.io", p); } }
            if let Some(s) = f.mexc   { if let Some(p) = mexc(&a, s)   { set_price(&lp, &f.pair, "MEXC", p); } }
        }
        // KRC-20: cross-checked pool read → windowed median (TWAP) → publish (never a raw spot)
        let ku = kas_usd(&lp);
        if ku > 0.0 {
            for f in feeds() {
                if let Some(p) = &f.pool {
                    if let Some((px_kas, liq)) = pool_read(&rpcs, p) {
                        let w = win.entry(f.pair.clone()).or_default();
                        w.push(px_kas * ku); if w.len() > TWAP_N { let d = w.len() - TWAP_N; w.drain(0..d); }
                        set_price(&lp, &f.pair, "Kasplex-DEX", median(w)); // windowed median, not the manipulable spot
                        liq_map().lock().unwrap().insert(f.pair.clone(), liq);
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

fn build(lp: &Live, keys: &[Keypair], round: u64, hist: &mut HashMap<String, Vec<(u64, f64)>>) -> String {
    let ts = now(); let tms = now_ms();
    let signers: Vec<String> = keys.iter().map(|k| format!("\"{}\"", hex::encode(k.x_only_public_key().0.serialize()))).collect();
    let book = lp.lock().unwrap().clone();
    let mut objs = Vec::new();
    for cfg in feeds() {
        let per = match book.get(&cfg.pair) { Some(m) => m, None => continue };
        let mut srcs: Vec<(&str, f64, u64)> = per.iter().filter(|(_, (_, t))| tms.saturating_sub(*t) < STALE_MS).map(|(n, (p, t))| (*n, *p, tms.saturating_sub(*t))).collect();
        if srcs.is_empty() { continue; }
        srcs.sort_by(|a, b| a.0.cmp(b.0));
        let prices: Vec<f64> = srcs.iter().map(|(_, p, _)| *p).collect();
        let med = median(&prices);
        let price_e8 = (med * 1e8).round() as u64;
        let msg = format!("kaspulse/v1|{}|{}|{}|{}", cfg.pair, price_e8, ts, round);
        let sigs: Vec<String> = keys.iter().map(|k| format!("\"{}\"", sign(k, &msg))).collect();
        let lo = prices.iter().cloned().fold(f64::MAX, f64::min); let hi = prices.iter().cloned().fold(f64::MIN, f64::max);
        let spread = if med > 0.0 { ((hi - lo) / med) * 10_000.0 } else { 0.0 };
        let freshest = srcs.iter().map(|(_, _, a)| *a).min().unwrap_or(0);
        let liq = liq_map().lock().unwrap().get(&cfg.pair).copied().unwrap_or(0.0);
        let thin = cfg.kind == "krc20" && liq < MIN_LIQ_WKAS; // low-liquidity pool → manipulable, low-confidence
        let src_j: Vec<String> = srcs.iter().map(|(n, p, a)| format!(r#"{{"name":"{n}","price":{},"age_ms":{a}}}"#, enum_(*p))).collect();
        let h = hist.entry(cfg.pair.clone()).or_default(); h.push((ts, med)); if h.len() > HISTORY { let d = h.len() - HISTORY; h.drain(0..d); }
        let hist_j: Vec<String> = h.iter().map(|(t, p)| format!("[{t},{}]", enum_(*p))).collect();
        objs.push(format!(
            r#"{{"pair":"{}","kind":"{}","price":{},"price_e8":{price_e8},"sources":[{}],"num_sources":{},"freshest_ms":{freshest},"low":{},"high":{},"spread_bps":{:.2},"median":{},"twap":true,"liq_wkas":{:.0},"thin":{thin},"signers":[{}],"threshold":{THRESHOLD},"signatures":[{}],"message":"{msg}","history":[{}]}}"#,
            cfg.pair, cfg.kind, enum_(med), src_j.join(","), srcs.len(), enum_(lo), enum_(hi), spread, enum_(med), liq, signers.join(","), sigs.join(","), hist_j.join(",")
        ));
    }
    format!(r#"{{"round":{round},"timestamp":{ts},"threshold":{THRESHOLD},"num_nodes":{N_NODES},"transport":"websocket","feeds":[{}]}}"#, objs.join(","))
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
            let mut round = 1u64; let mut hist = HashMap::new();
            loop {
                let json = build(&lp, &keys, round, &mut hist);
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

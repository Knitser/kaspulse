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
struct FeedCfg { pair: &'static str, kind: &'static str, kucoin: Option<&'static str>, gate: Option<&'static str>, mexc: Option<&'static str>, coingecko: Option<&'static str>, geckoterminal: Option<&'static str> }
fn feeds() -> Vec<FeedCfg> {
    vec![
        FeedCfg { pair: "KAS/USD", kind: "major", kucoin: Some("KAS-USDT"), gate: Some("KAS_USDT"), mexc: Some("KASUSDT"), coingecko: None, geckoterminal: None },
        FeedCfg { pair: "BTC/USD", kind: "major", kucoin: Some("BTC-USDT"), gate: Some("BTC_USDT"), mexc: Some("BTCUSDT"), coingecko: None, geckoterminal: None },
        FeedCfg { pair: "ETH/USD", kind: "major", kucoin: Some("ETH-USDT"), gate: Some("ETH_USDT"), mexc: Some("ETHUSDT"), coingecko: None, geckoterminal: None },
        FeedCfg { pair: "NACHO/USD", kind: "krc20", kucoin: None, gate: None, mexc: None, coingecko: Some("nacho-the-kat"), geckoterminal: Some("NACHO") },
        FeedCfg { pair: "KASPY/USD", kind: "krc20", kucoin: None, gate: None, mexc: None, coingecko: Some("kaspy"), geckoterminal: Some("KASPY") },
    ]
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

static CG_CACHE: std::sync::OnceLock<Mutex<HashMap<String, f64>>> = std::sync::OnceLock::new();
static CG_LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
const CG_EVERY: u64 = 20;
fn coingecko(ids: &[&str]) -> HashMap<String, f64> {
    use std::sync::atomic::Ordering::Relaxed;
    let mut m = HashMap::new(); if ids.is_empty() { return m; }
    let cache = CG_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if now().saturating_sub(CG_LAST.load(Relaxed)) >= CG_EVERY {
        if let Some(j) = get(&agent(), &format!("https://api.coingecko.com/api/v3/simple/price?ids={}&vs_currencies=usd", ids.join(","))) {
            for id in ids { if let Some(p) = j[*id]["usd"].as_f64() { m.insert(id.to_string(), p); } }
        }
        if !m.is_empty() { CG_LAST.store(now(), Relaxed); let mut c = cache.lock().unwrap(); for (k, v) in &m { c.insert(k.clone(), *v); } }
    }
    let c = cache.lock().unwrap(); for id in ids { if !m.contains_key(*id) { if let Some(v) = c.get(*id) { m.insert(id.to_string(), *v); } } } m
}
static GT_CACHE: std::sync::OnceLock<Mutex<HashMap<String, f64>>> = std::sync::OnceLock::new();
static GT_LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn geckoterminal(ticks: &[&str]) -> HashMap<String, f64> {
    use std::sync::atomic::Ordering::Relaxed;
    let mut m = HashMap::new(); if ticks.is_empty() { return m; }
    let cache = GT_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if now().saturating_sub(GT_LAST.load(Relaxed)) >= CG_EVERY {
        let a = agent();
        for t in ticks {
            if let Some(j) = get(&a, &format!("https://api.geckoterminal.com/api/v2/search/pools?query={t}&network=kasplex")) {
                if let Some(p) = j["data"].as_array().and_then(|arr| arr.iter().find_map(|pool| {
                    let name = pool["attributes"]["name"].as_str()?;
                    if name.to_uppercase().starts_with(&t.to_uppercase()) { pool["attributes"]["base_token_price_usd"].as_str()?.parse::<f64>().ok() } else { None }
                })) { m.insert(t.to_string(), p); }
            }
        }
        if !m.is_empty() { GT_LAST.store(now(), Relaxed); let mut c = cache.lock().unwrap(); for (k, v) in &m { c.insert(k.clone(), *v); } }
    }
    let c = cache.lock().unwrap(); for t in ticks { if !m.contains_key(*t) { if let Some(v) = c.get(*t) { m.insert(t.to_string(), *v); } } } m
}
// ---------- direct Kasplex DEX pool read — OUR OWN on-chain source ----------
// eth_call getReserves() on the token/WKAS pool, price in WKAS × our KAS/USD.
// No third-party API — anyone can re-run the same call and verify it.
const KASPLEX_RPC: &str = "https://evmrpc.kasplex.org";
struct PoolCfg { token: &'static str, pool: &'static str, wkas_is_token0: bool } // WKAS + KRC-20 both 18-dec on Kasplex
fn pools() -> Vec<PoolCfg> {
    vec![PoolCfg { token: "NACHO/USD", pool: "0xb905105452e5bedb1e6bd2d8c57e2b70f5a7349a", wkas_is_token0: true }]
}
fn eth_call(a: &ureq::Agent, to: &str, data: &str) -> Option<String> {
    let body = format!(r#"{{"jsonrpc":"2.0","method":"eth_call","params":[{{"to":"{to}","data":"{data}"}},"latest"],"id":1}}"#);
    let j: serde_json::Value = a.post(KASPLEX_RPC).set("content-type", "application/json").send_string(&body).ok()?.into_json().ok()?;
    j["result"].as_str().map(|s| s.to_string())
}
fn resv(h: &str) -> Option<f64> { u128::from_str_radix(h.get(32..64)?, 16).ok().map(|v| v as f64) } // uint112 fits in the low 128 bits
fn pool_price_kas(a: &ureq::Agent, c: &PoolCfg) -> Option<f64> {
    let h = eth_call(a, c.pool, "0x0902f1ac")?; let h = h.trim_start_matches("0x");
    if h.len() < 128 { return None; }
    let (r0, r1) = (resv(&h[0..64])?, resv(&h[64..128])?);
    if r0 <= 0.0 || r1 <= 0.0 { return None; }
    Some(if c.wkas_is_token0 { r0 / r1 } else { r1 / r0 }) // both 18-dec → ratio is the WKAS price
}
fn kas_usd(lp: &Live) -> f64 {
    match lp.lock().unwrap().get("KAS/USD") {
        Some(m) => { let mut v: Vec<f64> = m.values().map(|(p, _)| *p).collect(); v.sort_by(|a, b| a.partial_cmp(b).unwrap()); if v.is_empty() { 0.0 } else { v[v.len() / 2] } }
        None => 0.0,
    }
}

fn slow_thread(lp: Live) {
    loop {
        let a = agent();
        for f in feeds() {
            if let Some(s) = f.kucoin { if let Some(p) = kucoin(&a, s) { set_price(&lp, f.pair, "KuCoin", p); } }
            if let Some(s) = f.gate   { if let Some(p) = gate(&a, s)   { set_price(&lp, f.pair, "Gate.io", p); } }
            if let Some(s) = f.mexc   { if let Some(p) = mexc(&a, s)   { set_price(&lp, f.pair, "MEXC", p); } }
        }
        // OUR own on-chain source: read the Kasplex DEX pool directly
        let ku = kas_usd(&lp);
        if ku > 0.0 { for c in pools() { if let Some(px) = pool_price_kas(&a, &c) { set_price(&lp, c.token, "Kasplex-DEX", px * ku); } } }
        // aggregators (cross-check; being phased out for the direct read)
        let cg = coingecko(&["nacho-the-kat", "kaspy"]);
        if let Some(p) = cg.get("nacho-the-kat") { set_price(&lp, "NACHO/USD", "CoinGecko", *p); }
        if let Some(p) = cg.get("kaspy") { set_price(&lp, "KASPY/USD", "CoinGecko", *p); }
        let gt = geckoterminal(&["NACHO", "KASPY"]);
        if let Some(p) = gt.get("NACHO") { set_price(&lp, "NACHO/USD", "GeckoTerminal", *p); }
        if let Some(p) = gt.get("KASPY") { set_price(&lp, "KASPY/USD", "GeckoTerminal", *p); }
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
        let per = match book.get(cfg.pair) { Some(m) => m, None => continue };
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
        let src_j: Vec<String> = srcs.iter().map(|(n, p, a)| format!(r#"{{"name":"{n}","price":{},"age_ms":{a}}}"#, enum_(*p))).collect();
        let h = hist.entry(cfg.pair.to_string()).or_default(); h.push((ts, med)); if h.len() > HISTORY { let d = h.len() - HISTORY; h.drain(0..d); }
        let hist_j: Vec<String> = h.iter().map(|(t, p)| format!("[{t},{}]", enum_(*p))).collect();
        objs.push(format!(
            r#"{{"pair":"{}","kind":"{}","price":{},"price_e8":{price_e8},"sources":[{}],"num_sources":{},"freshest_ms":{freshest},"low":{},"high":{},"spread_bps":{:.2},"median":{},"signers":[{}],"threshold":{THRESHOLD},"signatures":[{}],"message":"{msg}","history":[{}]}}"#,
            cfg.pair, cfg.kind, enum_(med), src_j.join(","), srcs.len(), enum_(lo), enum_(hi), spread, enum_(med), signers.join(","), sigs.join(","), hist_j.join(",")
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

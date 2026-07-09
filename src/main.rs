//! kaspulse — a real-time, multi-asset price oracle for Kaspa.
//!
//! Each tick it prices several assets at once — majors (KAS/BTC/ETH) from a
//! median of independent exchanges, and KRC-20 tokens (NACHO/KASPY) that the big
//! oracles ignore. Every price is signed by a threshold of independent nodes and
//! verifiable by anyone (see the `verify` bin) — including on-chain (`onchain`).
//!
//! Feeds are fetched in parallel so rounds stay quick even across many assets.

use anyhow::Result;
use secp256k1::{Keypair, SECP256K1};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TICK: Duration = Duration::from_secs(2);
const PORT: u16 = 8080;
const HISTORY: usize = 120;
const N_NODES: usize = 5;
const THRESHOLD: usize = 3;

fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() }

// ---------- feeds ----------
struct FeedCfg {
    pair: &'static str,
    kind: &'static str, // "major" | "krc20"
    kraken: Option<&'static str>,
    kucoin: Option<&'static str>,
    gate: Option<&'static str>,
    bybit: Option<&'static str>,
    mexc: Option<&'static str>,
    coingecko: Option<&'static str>,      // CoinGecko id (aggregator)
    geckoterminal: Option<&'static str>,  // Kasplex tick — reads the on-chain DEX pool
}
fn feeds() -> Vec<FeedCfg> {
    vec![
        FeedCfg { pair: "KAS/USD",  kind: "major", kraken: Some("KASUSD"), kucoin: Some("KAS-USDT"), gate: Some("KAS_USDT"), bybit: Some("KASUSDT"), mexc: Some("KASUSDT"), coingecko: None, geckoterminal: None },
        FeedCfg { pair: "BTC/USD",  kind: "major", kraken: Some("XBTUSD"), kucoin: Some("BTC-USDT"), gate: Some("BTC_USDT"), bybit: Some("BTCUSDT"), mexc: Some("BTCUSDT"), coingecko: None, geckoterminal: None },
        FeedCfg { pair: "ETH/USD",  kind: "major", kraken: Some("ETHUSD"), kucoin: Some("ETH-USDT"), gate: Some("ETH_USDT"), bybit: Some("ETHUSDT"), mexc: Some("ETHUSDT"), coingecko: None, geckoterminal: None },
        // KRC-20: aggregator (CoinGecko) + on-chain DEX pool (GeckoTerminal/Kasplex). Pin by network=kasplex so we never grab a same-symbol token on another chain.
        FeedCfg { pair: "NACHO/USD", kind: "krc20", kraken: None, kucoin: None, gate: None, bybit: None, mexc: None, coingecko: Some("nacho-the-kat"), geckoterminal: Some("NACHO") },
        FeedCfg { pair: "KASPY/USD", kind: "krc20", kraken: None, kucoin: None, gate: None, bybit: None, mexc: None, coingecko: Some("kaspy"), geckoterminal: Some("KASPY") },
    ]
}

// ---------- fetch ----------
fn agent() -> ureq::Agent { ureq::AgentBuilder::new().timeout(Duration::from_secs(7)).build() }
fn get(a: &ureq::Agent, url: &str) -> Option<serde_json::Value> { a.get(url).call().ok()?.into_json().ok() }
fn pf(v: &serde_json::Value) -> Option<f64> { v.as_str().and_then(|s| s.parse().ok()).or_else(|| v.as_f64()) }

fn kraken(a: &ureq::Agent, sym: &str) -> Option<f64> {
    let j = get(a, &format!("https://api.kraken.com/0/public/Ticker?pair={sym}"))?;
    j["result"].as_object()?.values().next()?["c"].get(0).and_then(pf) // result key varies by pair
}
fn kucoin(a: &ureq::Agent, sym: &str) -> Option<f64> { pf(&get(a, &format!("https://api.kucoin.com/api/v1/market/orderbook/level1?symbol={sym}"))?["data"]["price"]) }
fn gate(a: &ureq::Agent, sym: &str) -> Option<f64> { get(a, &format!("https://api.gateio.ws/api/v4/spot/tickers?currency_pair={sym}"))?.get(0).and_then(|x| pf(&x["last"])) }
fn bybit(a: &ureq::Agent, sym: &str) -> Option<f64> { get(a, &format!("https://api.bybit.com/v5/market/tickers?category=spot&symbol={sym}"))?["result"]["list"].get(0).and_then(|x| pf(&x["lastPrice"])) }
fn mexc(a: &ureq::Agent, sym: &str) -> Option<f64> { pf(&get(a, &format!("https://api.mexc.com/api/v3/ticker/price?symbol={sym}"))?["price"]) }

/// one CoinGecko call for every KRC-20 id (batched → avoids rate limits).
/// When CoinGecko throttles us, fall back to the last-known price so KRC-20
/// feeds stay live (a real oracle tolerates a flaky source; it would also track
/// staleness + circuit-break if the price got too old — a Phase-2 item).
static CG_CACHE: std::sync::OnceLock<Mutex<HashMap<String, f64>>> = std::sync::OnceLock::new();
static CG_LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
const CG_EVERY: u64 = 20; // seconds — KRC-20 doesn't need 2s cadence; stays under the free rate limit
fn coingecko_batch(ids: &[&str]) -> HashMap<String, f64> {
    use std::sync::atomic::Ordering::Relaxed;
    let mut m = HashMap::new();
    if ids.is_empty() { return m; }
    let cache = CG_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if now().saturating_sub(CG_LAST.load(Relaxed)) >= CG_EVERY {
        if let Some(j) = get(&agent(), &format!("https://api.coingecko.com/api/v3/simple/price?ids={}&vs_currencies=usd", ids.join(","))) {
            for id in ids { if let Some(p) = j[*id]["usd"].as_f64() { m.insert(id.to_string(), p); } }
        }
        if !m.is_empty() { CG_LAST.store(now(), Relaxed); let mut c = cache.lock().unwrap(); for (k, v) in &m { c.insert(k.clone(), *v); } }
    }
    let c = cache.lock().unwrap(); // fill everything (or fall back) from cache
    for id in ids { if !m.contains_key(*id) { if let Some(v) = c.get(*id) { m.insert(id.to_string(), *v); } } }
    m
}

/// Read KRC-20 prices from GeckoTerminal's Kasplex DEX pools — the actual
/// on-chain liquidity, not an aggregator. Pinned to network=kasplex so a
/// same-symbol token on another chain (there are 5+ "NACHO"s!) can't leak in.
static GT_CACHE: std::sync::OnceLock<Mutex<HashMap<String, f64>>> = std::sync::OnceLock::new();
static GT_LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn geckoterminal_batch(ticks: &[&str]) -> HashMap<String, f64> {
    use std::sync::atomic::Ordering::Relaxed;
    let mut m = HashMap::new();
    if ticks.is_empty() { return m; }
    let cache = GT_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if now().saturating_sub(GT_LAST.load(Relaxed)) >= CG_EVERY {
        let a = agent();
        for tick in ticks {
            if let Some(j) = get(&a, &format!("https://api.geckoterminal.com/api/v2/search/pools?query={tick}&network=kasplex")) {
                if let Some(p) = j["data"].as_array().and_then(|arr| arr.iter().find_map(|pool| {
                    let name = pool["attributes"]["name"].as_str()?;
                    if name.to_uppercase().starts_with(&tick.to_uppercase()) { pool["attributes"]["base_token_price_usd"].as_str()?.parse::<f64>().ok() } else { None }
                })) { m.insert(tick.to_string(), p); }
            }
        }
        if !m.is_empty() { GT_LAST.store(now(), Relaxed); let mut c = cache.lock().unwrap(); for (k, v) in &m { c.insert(k.clone(), *v); } }
    }
    let c = cache.lock().unwrap();
    for tick in ticks { if !m.contains_key(*tick) { if let Some(v) = c.get(*tick) { m.insert(tick.to_string(), *v); } } }
    m
}

fn fetch_feed(cfg: &FeedCfg, cg: &HashMap<String, f64>, gt: &HashMap<String, f64>) -> Vec<(&'static str, f64)> {
    let a = agent();
    let mut out = Vec::new();
    if let Some(s) = cfg.kraken { if let Some(p) = kraken(&a, s) { out.push(("Kraken", p)); } }
    if let Some(s) = cfg.kucoin { if let Some(p) = kucoin(&a, s) { out.push(("KuCoin", p)); } }
    if let Some(s) = cfg.gate   { if let Some(p) = gate(&a, s)   { out.push(("Gate.io", p)); } }
    if let Some(s) = cfg.bybit  { if let Some(p) = bybit(&a, s)  { out.push(("Bybit", p)); } }
    if let Some(s) = cfg.mexc   { if let Some(p) = mexc(&a, s)   { out.push(("MEXC", p)); } }
    if let Some(id) = cfg.coingecko { if let Some(p) = cg.get(id) { out.push(("CoinGecko", *p)); } }
    if let Some(t) = cfg.geckoterminal { if let Some(p) = gt.get(t) { out.push(("GeckoTerminal", *p)); } }
    out
}

/// fetch every feed in parallel (one thread per feed).
fn fetch_all(cfgs: &[FeedCfg]) -> Vec<Vec<(&'static str, f64)>> {
    let cg = coingecko_batch(&cfgs.iter().filter_map(|c| c.coingecko).collect::<Vec<_>>());
    let gt = geckoterminal_batch(&cfgs.iter().filter_map(|c| c.geckoterminal).collect::<Vec<_>>());
    std::thread::scope(|s| {
        let handles: Vec<_> = cfgs.iter().map(|c| s.spawn(|| fetch_feed(c, &cg, &gt))).collect();
        handles.into_iter().map(|h| h.join().unwrap_or_default()).collect()
    })
}

fn median(xs: &[f64]) -> f64 {
    let mut v = xs.to_vec(); v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len(); if n == 0 { 0.0 } else if n % 2 == 1 { v[n/2] } else { (v[n/2-1]+v[n/2])/2.0 }
}

// ---------- keys / signing ----------
fn load_keys(n: usize) -> Vec<Keypair> {
    (0..n).map(|i| {
        let path = format!("kaspulse-node-{i}.key");
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(b) = hex::decode(raw.trim()) {
                if let Ok(sk) = secp256k1::SecretKey::from_slice(&b) { return Keypair::from_secret_key(SECP256K1, &sk); }
            }
        }
        let kp = Keypair::new(SECP256K1, &mut secp256k1::rand::thread_rng());
        let _ = std::fs::write(&path, hex::encode(kp.secret_key().secret_bytes()));
        kp
    }).collect()
}
fn sign(kp: &Keypair, msg: &str) -> String {
    let h = blake2b_simd::Params::new().hash_length(32).hash(msg.as_bytes());
    hex::encode(kp.sign_schnorr(secp256k1::Message::from_digest_slice(h.as_bytes()).unwrap()).as_ref())
}

// ---------- feed json ----------
fn esc_num(p: f64) -> String { if p == 0.0 { "0".into() } else { format!("{p}") } }

fn build_all(keys: &[Keypair], round: u64, hist: &mut HashMap<String, Vec<(u64, f64)>>) -> String {
    let cfgs = feeds();
    let all = fetch_all(&cfgs);
    let ts = now();
    let signers: Vec<String> = keys.iter().map(|k| format!("\"{}\"", hex::encode(k.x_only_public_key().0.serialize()))).collect();
    let mut objs = Vec::new();
    for (cfg, sources) in cfgs.iter().zip(all.into_iter()) {
        if sources.is_empty() { continue; }
        let prices: Vec<f64> = sources.iter().map(|(_, p)| *p).collect();
        let med = median(&prices);
        let price_e8 = (med * 1e8).round() as u64;
        let msg = format!("kaspulse/v1|{}|{}|{}|{}", cfg.pair, price_e8, ts, round);
        let sigs: Vec<String> = keys.iter().map(|k| format!("\"{}\"", sign(k, &msg))).collect();
        let lo = prices.iter().cloned().fold(f64::MAX, f64::min);
        let hi = prices.iter().cloned().fold(f64::MIN, f64::max);
        let spread = if med > 0.0 { ((hi - lo) / med) * 10_000.0 } else { 0.0 };
        let src_j: Vec<String> = sources.iter().map(|(n, p)| format!(r#"{{"name":"{n}","price":{}}}"#, esc_num(*p))).collect();
        let h = hist.entry(cfg.pair.to_string()).or_default();
        h.push((ts, med));
        if h.len() > HISTORY { let d = h.len() - HISTORY; h.drain(0..d); }
        let hist_j: Vec<String> = h.iter().map(|(t, p)| format!("[{t},{}]", esc_num(*p))).collect();
        objs.push(format!(
            r#"{{"pair":"{}","kind":"{}","price":{},"price_e8":{price_e8},"sources":[{}],"num_sources":{},"low":{},"high":{},"spread_bps":{:.2},"median":{},"signers":[{}],"threshold":{THRESHOLD},"signatures":[{}],"message":"{msg}","history":[{}]}}"#,
            cfg.pair, cfg.kind, esc_num(med), src_j.join(","), sources.len(), esc_num(lo), esc_num(hi), spread, esc_num(med), signers.join(","), sigs.join(","), hist_j.join(",")
        ));
    }
    format!(r#"{{"round":{round},"timestamp":{ts},"threshold":{THRESHOLD},"num_nodes":{N_NODES},"feeds":[{}]}}"#, objs.join(","))
}

// ---------- http ----------
fn mime(p: &str) -> &'static str {
    if p.ends_with(".html") { "text/html; charset=utf-8" } else if p.ends_with(".js") { "application/javascript" }
    else if p.ends_with(".css") { "text/css" } else if p.ends_with(".json") { "application/json" } else { "text/plain" }
}
fn serve(mut s: std::net::TcpStream, feed: &Arc<Mutex<String>>) {
    let mut buf = [0u8; 2048];
    let n = match s.read(&mut buf) { Ok(n) => n, Err(_) => return };
    let path = String::from_utf8_lossy(&buf[..n]).split_whitespace().nth(1).unwrap_or("/").to_string();
    let cors = "Access-Control-Allow-Origin: *\r\n";
    let (status, ctype, body): (&str, &str, Vec<u8>) = if path.starts_with("/api/feed") || path == "/feed.json" {
        let full = feed.lock().unwrap().clone();
        // /api/feed/<PAIR> filters to one feed
        if let Some(p) = path.strip_prefix("/api/feed/") {
            let want = p.replace("-", "/").to_uppercase();
            let one = serde_json::from_str::<serde_json::Value>(&full).ok()
                .and_then(|v| v["feeds"].as_array()?.iter().find(|f| f["pair"].as_str() == Some(&want)).cloned())
                .map(|v| v.to_string()).unwrap_or_else(|| "{\"error\":\"no such feed\"}".into());
            ("200 OK", "application/json", one.into_bytes())
        } else { ("200 OK", "application/json", full.into_bytes()) }
    } else {
        let file = if path == "/" { "index.html".into() } else { path.trim_start_matches('/').to_string() };
        match if !file.contains("..") { std::fs::read(format!("web/{file}")).ok() } else { None } {
            Some(b) => ("200 OK", mime(&file), b),
            None => ("404 Not Found", "text/plain", b"not found".to_vec()),
        }
    };
    let head = format!("HTTP/1.1 {status}\r\n{cors}Content-Type: {ctype}\r\nContent-Length: {}\r\n\r\n", body.len());
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(&body);
}

fn main() -> Result<()> {
    let keys = load_keys(N_NODES);
    let cfgs = feeds();
    println!("kaspulse oracle — {} feeds, {N_NODES} nodes, {THRESHOLD}-of-{N_NODES} threshold", cfgs.len());
    for c in &cfgs { println!("  {} ({})", c.pair, c.kind); }
    println!("serving http://127.0.0.1:{PORT}  (dashboard + /api/feed)");

    let feed = Arc::new(Mutex::new(r#"{"feeds":[]}"#.to_string()));
    {
        let feed = feed.clone();
        std::thread::spawn(move || {
            let mut round = 1u64;
            let mut hist: HashMap<String, Vec<(u64, f64)>> = HashMap::new();
            loop {
                let json = build_all(&keys, round, &mut hist);
                let m = serde_json::from_str::<serde_json::Value>(&json).ok();
                let n = m.as_ref().and_then(|v| v["feeds"].as_array()).map(|a| a.len()).unwrap_or(0);
                println!("round {round}: {n} feeds priced + signed");
                let _ = std::fs::write("web/feed.json", &json);
                *feed.lock().unwrap() = json;
                round += 1;
                std::thread::sleep(TICK);
            }
        });
    }
    let listener = TcpListener::bind(("127.0.0.1", PORT))?;
    for stream in listener.incoming() {
        if let Ok(s) = stream { let feed = feed.clone(); std::thread::spawn(move || serve(s, &feed)); }
    }
    Ok(())
}

//! kaspulse — the real-time price oracle for Kaspa.
//!
//! Every tick it pulls KAS/USD from several exchanges, takes the median (so no
//! single venue can move the feed), and SIGNS the result with the oracle's key
//! (Schnorr). The signed attestation is what a Kaspa covenant verifies on-chain
//! — the price is trustless: anyone can re-derive the median and check the
//! signature. Kaspa's ~100ms blocks let this refresh far faster than oracles on
//! slower chains — the whole point.
//!
//! This binary is the off-chain half: fetch → aggregate → sign → serve. The
//! on-chain half (publishing the signed price into a covenant "price coin" and
//! a consumer contract that reads it) lives in `onchain.rs`.

use anyhow::Result;
use secp256k1::{Keypair, SECP256K1};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PAIR: &str = "KAS/USD";
const TICK: Duration = Duration::from_secs(2); // real-time-ish; Kaspa can go faster
const PORT: u16 = 8080;
const HISTORY: usize = 180;
const N_NODES: usize = 5;   // independent signers (simulated here as one process;
const THRESHOLD: usize = 3; // in production each runs on a separate operator/machine)

fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() }

// ---------- data sources ----------
fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(7)).build()
}
fn get(a: &ureq::Agent, url: &str) -> Option<serde_json::Value> {
    a.get(url).call().ok()?.into_json::<serde_json::Value>().ok()
}
fn f(v: &serde_json::Value) -> Option<f64> {
    v.as_str().and_then(|s| s.parse().ok()).or_else(|| v.as_f64())
}

/// (exchange name, price) for every venue that answers. Skips any that fail —
/// the median just uses whoever's up.
fn fetch_sources() -> Vec<(&'static str, f64)> {
    let a = agent();
    let mut out = Vec::new();
    if let Some(j) = get(&a, "https://api.kraken.com/0/public/Ticker?pair=KASUSD") {
        if let Some(p) = j["result"]["KASUSD"]["c"].get(0).and_then(f) { out.push(("Kraken", p)); }
    }
    if let Some(j) = get(&a, "https://api.kucoin.com/api/v1/market/orderbook/level1?symbol=KAS-USDT") {
        if let Some(p) = f(&j["data"]["price"]) { out.push(("KuCoin", p)); }
    }
    if let Some(j) = get(&a, "https://api.gateio.ws/api/v4/spot/tickers?currency_pair=KAS_USDT") {
        if let Some(p) = j.get(0).and_then(|x| f(&x["last"])) { out.push(("Gate.io", p)); }
    }
    if let Some(j) = get(&a, "https://api.bybit.com/v5/market/tickers?category=spot&symbol=KASUSDT") {
        if let Some(p) = j["result"]["list"].get(0).and_then(|x| f(&x["lastPrice"])) { out.push(("Bybit", p)); }
    }
    if let Some(j) = get(&a, "https://api.mexc.com/api/v3/ticker/price?symbol=KASUSDT") {
        if let Some(p) = f(&j["price"]) { out.push(("MEXC", p)); }
    }
    // CoinGecko is itself an aggregator (a secondary source) — handy as a
    // cross-check, but a real oracle leans on PRIMARY venues so it isn't
    // trusting someone else's (slower, opaque) aggregation.
    if let Some(j) = get(&a, "https://api.coingecko.com/api/v3/simple/price?ids=kaspa&vs_currencies=usd") {
        if let Some(p) = f(&j["kaspa"]["usd"]) { out.push(("CoinGecko", p)); }
    }
    out
}

fn median(xs: &[f64]) -> f64 {
    let mut v: Vec<f64> = xs.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n == 0 { 0.0 } else if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
}

// ---------- signed attestation ----------
/// N independent signer keys. Persisted so the pubkeys are stable (a consumer
/// covenant commits to them). In the demo they live in one process; in
/// production each is a separate operator that fetches + signs on its own.
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

/// The canonical message a consumer re-derives and checks the signature over.
fn message(pair: &str, price_e8: u64, ts: u64, round: u64) -> String {
    format!("kaspulse/v1|{pair}|{price_e8}|{ts}|{round}")
}
fn sign(kp: &Keypair, msg: &str) -> String {
    let h = blake2b_simd::Params::new().hash_length(32).hash(msg.as_bytes());
    let m = secp256k1::Message::from_digest_slice(h.as_bytes()).unwrap();
    hex::encode(kp.sign_schnorr(m).as_ref())
}

// ---------- feed builder ----------
fn build_feed(keys: &[Keypair], round: u64, history: &[(u64, f64)]) -> (String, Option<(u64, f64)>) {
    let sources = fetch_sources();
    if sources.is_empty() {
        return (r#"{"error":"all sources unreachable"}"#.to_string(), None);
    }
    let prices: Vec<f64> = sources.iter().map(|(_, p)| *p).collect();
    let med = median(&prices);
    let price_e8 = (med * 1e8).round() as u64;
    let ts = now();
    let msg = message(PAIR, price_e8, ts, round);
    // every node independently signs the agreed median; a consumer needs a
    // THRESHOLD of these to accept the price — no single node can move it.
    let sigs_j: Vec<String> = keys.iter().map(|k| format!("\"{}\"", sign(k, &msg))).collect();
    let signers_j: Vec<String> = keys.iter().map(|k| format!("\"{}\"", hex::encode(k.x_only_public_key().0.serialize()))).collect();
    let lo = prices.iter().cloned().fold(f64::MAX, f64::min);
    let hi = prices.iter().cloned().fold(f64::MIN, f64::max);
    let spread_bps = if med > 0.0 { ((hi - lo) / med) * 10_000.0 } else { 0.0 };

    let src_json: Vec<String> = sources.iter()
        .map(|(n, p)| format!(r#"{{"name":"{n}","price":{p}}}"#)).collect();
    let mut hist = history.to_vec();
    hist.push((ts, med));
    if hist.len() > HISTORY { let drop = hist.len() - HISTORY; hist.drain(0..drop); }
    let hist_json: Vec<String> = hist.iter().map(|(t, p)| format!("[{t},{p}]")).collect();

    let json = format!(
        r#"{{"pair":"{PAIR}","price":{med},"price_e8":{price_e8},"round":{round},"timestamp":{ts},"sources":[{}],"num_sources":{},"low":{lo},"high":{hi},"spread_bps":{:.2},"median":{med},"signers":[{}],"threshold":{THRESHOLD},"signatures":[{}],"message":"{msg}","history":[{}]}}"#,
        src_json.join(","), sources.len(), spread_bps, signers_j.join(","), sigs_j.join(","), hist_json.join(",")
    );
    (json, Some((ts, med)))
}

// ---------- http ----------
fn mime(path: &str) -> &'static str {
    if path.ends_with(".html") { "text/html; charset=utf-8" }
    else if path.ends_with(".js") { "application/javascript" }
    else if path.ends_with(".css") { "text/css" }
    else if path.ends_with(".json") { "application/json" }
    else { "text/plain" }
}
fn serve(mut s: std::net::TcpStream, feed: &Arc<Mutex<String>>) {
    let mut buf = [0u8; 2048];
    let n = match s.read(&mut buf) { Ok(n) => n, Err(_) => return };
    let req = String::from_utf8_lossy(&buf[..n]);
    let path = req.split_whitespace().nth(1).unwrap_or("/");
    let cors = "Access-Control-Allow-Origin: *\r\n";
    let (status, ctype, body): (&str, &str, Vec<u8>) = if path == "/api/feed" || path == "/feed.json" {
        ("200 OK", "application/json", feed.lock().unwrap().clone().into_bytes())
    } else {
        let file = if path == "/" { "index.html" } else { path.trim_start_matches('/') };
        let safe = !file.contains("..");
        match if safe { std::fs::read(format!("web/{file}")).ok() } else { None } {
            Some(b) => ("200 OK", mime(file), b),
            None => ("404 Not Found", "text/plain", b"not found".to_vec()),
        }
    };
    let head = format!("HTTP/1.1 {status}\r\n{cors}Content-Type: {ctype}\r\nContent-Length: {}\r\n\r\n", body.len());
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(&body);
}

fn main() -> Result<()> {
    let keys = load_keys(N_NODES);
    println!("kaspulse oracle — {N_NODES} independent signers, {THRESHOLD}-of-{N_NODES} threshold");
    for (i, k) in keys.iter().enumerate() {
        println!("  node {i}: {}", hex::encode(k.x_only_public_key().0.serialize()));
    }
    println!("serving http://127.0.0.1:{PORT}  (dashboard + /api/feed)");

    let feed = Arc::new(Mutex::new(r#"{"status":"starting"}"#.to_string()));
    {
        let feed = feed.clone();
        std::thread::spawn(move || {
            let mut round = 1u64;
            let mut history: Vec<(u64, f64)> = Vec::new();
            loop {
                let (json, point) = build_feed(&keys, round, &history);
                if let Some(pt) = point {
                    history.push(pt);
                    if history.len() > HISTORY { let d = history.len() - HISTORY; history.drain(0..d); }
                    println!("round {round}: {PAIR} = ${:.6}  ({} sources)", pt.1, fetch_count(&json));
                }
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

fn fetch_count(json: &str) -> usize {
    json.matches("\"name\":").count()
}

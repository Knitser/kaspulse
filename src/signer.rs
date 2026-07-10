//! kaspulse signer — a STANDALONE oracle signer daemon.
//!
//! The reality-check's #1 fatal gap was that the 5 "nodes" run in one process:
//! the threshold is cosmetic when the faults are perfectly correlated. This is
//! the fix — each INDEPENDENT operator runs one `signer` on their OWN machine,
//! with their OWN key, fetching the market THEMSELVES. An aggregator (or the
//! `oracle`) polls each operator's /attest endpoint and assembles the k-of-n
//! feed. Now compromising the threshold means compromising k independent people.
//!
//! Run: cargo run --bin signer -- [key_path] [port]   (defaults: signer.key 9099)
//! Then: curl http://localhost:9099/attest

use secp256k1::{Keypair, Message, SECP256K1};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() }
fn agent() -> ureq::Agent { ureq::AgentBuilder::new().timeout(Duration::from_secs(7)).build() }
fn get(a: &ureq::Agent, u: &str) -> Option<serde_json::Value> { a.get(u).call().ok()?.into_json().ok() }
fn pf(v: &serde_json::Value) -> Option<f64> { v.as_str().and_then(|s| s.parse().ok()).or_else(|| v.as_f64()) }
fn median(mut v: Vec<f64>) -> f64 { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); let n = v.len(); if n == 0 { 0.0 } else if n % 2 == 1 { v[n/2] } else { (v[n/2-1]+v[n/2])/2.0 } }
fn mant_expo(p: f64) -> (u64, i32) {
    if p <= 0.0 || !p.is_finite() { return (0, 0); }
    let mut e = p.log10().floor() as i32 - 8; let mut m = (p / 10f64.powi(e)).round() as u64;
    if m >= 1_000_000_000 { m /= 10; e += 1; } (m, e)
}
fn sign(kp: &Keypair, msg: &str) -> String {
    let h = blake2b_simd::Params::new().hash_length(32).hash(msg.as_bytes());
    hex::encode(kp.sign_schnorr(Message::from_digest_slice(h.as_bytes()).unwrap()).as_ref())
}

// each operator fetches the market INDEPENDENTLY (their own view, their own sources)
fn fetch(pair: &str) -> Vec<f64> {
    let a = agent();
    let (kr, kc, gt, by, mx) = match pair {
        "KAS/USD" => ("KASUSD", "KAS-USDT", "KAS_USDT", "KASUSDT", "KASUSDT"),
        "BTC/USD" => ("XBTUSD", "BTC-USDT", "BTC_USDT", "BTCUSDT", "BTCUSDT"),
        _ => ("ETHUSD", "ETH-USDT", "ETH_USDT", "ETHUSDT", "ETHUSDT"),
    };
    let mut ps = Vec::new();
    if let Some(j) = get(&a, &format!("https://api.kraken.com/0/public/Ticker?pair={kr}")) { if let Some(p) = j["result"].as_object().and_then(|o| o.values().next()).and_then(|t| t["c"].get(0)).and_then(pf) { ps.push(p); } }
    if let Some(j) = get(&a, &format!("https://api.kucoin.com/api/v1/market/orderbook/level1?symbol={kc}")) { if let Some(p) = pf(&j["data"]["price"]) { ps.push(p); } }
    if let Some(j) = get(&a, &format!("https://api.gateio.ws/api/v4/spot/tickers?currency_pair={gt}")) { if let Some(p) = j.get(0).and_then(|x| pf(&x["last"])) { ps.push(p); } }
    if let Some(j) = get(&a, &format!("https://api.bybit.com/v5/market/tickers?category=spot&symbol={by}")) { if let Some(p) = j["result"]["list"].get(0).and_then(|x| pf(&x["lastPrice"])) { ps.push(p); } }
    if let Some(j) = get(&a, &format!("https://api.mexc.com/api/v3/ticker/price?symbol={mx}")) { if let Some(p) = pf(&j["price"]) { ps.push(p); } }
    ps
}

fn main() {
    let key_path = std::env::args().nth(1).unwrap_or_else(|| "signer.key".into());
    let port: u16 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(9099);
    let kp = load_key(&key_path);
    let pubkey = hex::encode(kp.x_only_public_key().0.serialize());
    println!("kaspulse signer — operator key {pubkey}");
    println!("serving http://127.0.0.1:{port}/attest  (poll this from an aggregator)");

    let out = Arc::new(Mutex::new("[]".to_string()));
    {
        let (out, kp) = (out.clone(), kp);
        std::thread::spawn(move || {
            let mut round = 1u64;
            loop {
                let ts = now();
                let mut objs = Vec::new();
                for pair in ["KAS/USD", "BTC/USD", "ETH/USD"] {
                    let m = median(fetch(pair));
                    if m <= 0.0 { continue; }
                    let (mant, expo) = mant_expo(m);
                    let msg = format!("kaspulse/v2|{pair}|{mant}|{expo}|{ts}|{round}");
                    objs.push(format!(r#"{{"pair":"{pair}","mant":{mant},"expo":{expo},"ts":{ts},"round":{round},"signer":"{pubkey}","signature":"{}","message":"{msg}"}}"#, sign(&kp, &msg)));
                }
                *out.lock().unwrap() = format!("[{}]", objs.join(","));
                println!("round {round}: signed {} pairs", objs.len());
                round += 1;
                std::thread::sleep(Duration::from_secs(2));
            }
        });
    }
    let l = TcpListener::bind(("127.0.0.1", port)).expect("bind");
    for s in l.incoming() {
        if let Ok(mut s) = s {
            let out = out.clone();
            std::thread::spawn(move || {
                let mut b = [0u8; 512]; let _ = s.read(&mut b);
                let body = out.lock().unwrap().clone();
                let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes());
            });
        }
    }
}

fn load_key(path: &str) -> Keypair {
    if let Ok(raw) = std::fs::read_to_string(path) {
        if let Ok(b) = hex::decode(raw.trim()) { if let Ok(sk) = secp256k1::SecretKey::from_slice(&b) { return Keypair::from_secret_key(SECP256K1, &sk); } }
    }
    let kp = Keypair::new(SECP256K1, &mut secp256k1::rand::thread_rng());
    let _ = std::fs::write(path, hex::encode(kp.secret_key().secret_bytes()));
    kp
}

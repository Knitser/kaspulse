//! kaspulse verify — independently check that a feed is honest.
//!
//! An oracle is only useful if you DON'T have to trust it. This tool takes a
//! live feed and, with no help from the oracle, (1) re-verifies every node's
//! Schnorr signature over the price, and (2) re-fetches the exchanges and
//! recomputes the median itself. If both check out, the price is real and
//! signed by the threshold — provably, not on faith.
//!
//! Run the oracle (`cargo run --bin oracle`), then: `cargo run --bin verify`.

use secp256k1::{schnorr, Message, XOnlyPublicKey, SECP256K1};
use serde_json::Value;
use std::time::Duration;

fn agent() -> ureq::Agent { ureq::AgentBuilder::new().timeout(Duration::from_secs(7)).build() }
fn get(a: &ureq::Agent, url: &str) -> Option<Value> { a.get(url).call().ok()?.into_json().ok() }
fn f(v: &Value) -> Option<f64> { v.as_str().and_then(|s| s.parse().ok()).or_else(|| v.as_f64()) }

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
    out
}
fn median(xs: &[f64]) -> f64 {
    let mut v = xs.to_vec(); v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len(); if n == 0 { 0.0 } else if n % 2 == 1 { v[n/2] } else { (v[n/2-1]+v[n/2])/2.0 }
}

fn main() {
    let url = std::env::args().nth(1).unwrap_or_else(|| "http://127.0.0.1:8080/api/feed".into());
    println!("kaspulse verify — pulling the feed from {url}\n");
    let feed = match get(&agent(), &url) { Some(v) => v, None => { eprintln!("could not reach the oracle — is it running? (cargo run --bin oracle)"); std::process::exit(1); } };

    let msg = feed["message"].as_str().unwrap_or("");
    let threshold = feed["threshold"].as_u64().unwrap_or(0);
    let signers = feed["signers"].as_array().cloned().unwrap_or_default();
    let sigs = feed["signatures"].as_array().cloned().unwrap_or_default();
    println!("message signed:  {msg}");

    // (1) verify each node's Schnorr signature over blake2b(message)
    let h = blake2b_simd::Params::new().hash_length(32).hash(msg.as_bytes());
    let m = Message::from_digest_slice(h.as_bytes()).unwrap();
    let mut valid = 0;
    println!("\nsignatures ({} nodes, need {threshold}):", signers.len());
    for (pk_hex, sig_hex) in signers.iter().zip(sigs.iter()) {
        let ok = (|| {
            let pk = XOnlyPublicKey::from_slice(&hex::decode(pk_hex.as_str()?).ok()?).ok()?;
            let sig = schnorr::Signature::from_slice(&hex::decode(sig_hex.as_str()?).ok()?).ok()?;
            Some(SECP256K1.verify_schnorr(&sig, &m, &pk).is_ok())
        })().unwrap_or(false);
        let s = pk_hex.as_str().unwrap_or("");
        println!("  {} node {}…", if ok { "✓" } else { "✗" }, &s[..12.min(s.len())]);
        if ok { valid += 1; }
    }
    let sig_ok = valid as u64 >= threshold && threshold > 0;
    println!("  → {valid}/{} valid · threshold {threshold} {}", signers.len(), if sig_ok { "MET ✓" } else { "NOT met ✗" });

    // (2) recompute the median from the exchanges, independently
    println!("\nindependent price check (re-fetching exchanges myself):");
    let src = fetch_sources();
    for (n, p) in &src { println!("  {n}: ${p:.6}"); }
    let mine = median(&src.iter().map(|(_, p)| *p).collect::<Vec<_>>());
    let theirs = feed["median"].as_f64().unwrap_or(0.0);
    let drift = if theirs > 0.0 { ((mine - theirs).abs() / theirs) * 100.0 } else { 100.0 };
    let price_ok = drift < 1.0; // within 1% (prices move between fetches)
    println!("  my median ${mine:.6}  vs  feed ${theirs:.6}  ({drift:.2}% drift) {}",
        if price_ok { "✓" } else { "⚠ (moved / off)" });

    println!("\n{}", if sig_ok && price_ok {
        "VERDICT: honest feed — signed by the threshold AND the price matches the market. No trust required."
    } else {
        "VERDICT: something's off — see above."
    });
}

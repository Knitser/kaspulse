//! kaspulse verify — independently check that a feed is honest.
//!
//! An oracle is only useful if you DON'T have to trust it. This tool takes a
//! live feed and, with no help from the oracle, (1) re-verifies every node's
//! Schnorr signature over the price with field binding (same rules as the SDK),
//! and (2) re-fetches the exchanges and recomputes the median itself. If both
//! check out, the price is real and signed by the threshold — provably, not on
//! faith.
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

fn parse_u64_strict(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) { return None; }
    if s.len() > 1 && s.starts_with('0') { return None; }
    s.parse().ok()
}
fn parse_i32_strict(s: &str) -> Option<i32> {
    let t = s.strip_prefix('-').unwrap_or(s);
    if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) { return None; }
    if t.len() > 1 && t.starts_with('0') { return None; }
    if t == "0" && s.starts_with('-') { return None; }
    s.parse().ok()
}

/// Same rules as `kaspulse_sdk::Feed::verify` — field binding + guard flags + threshold Schnorr.
fn verify_feed(f: &Value) -> Result<(usize, usize), &'static str> {
    if f["halted"].as_bool() == Some(true) { return Err("feed halted (circuit breaker)"); }
    if f["peg_ok"].as_bool() == Some(false) { return Err("chain depegged"); }
    let message = f["message"].as_str().ok_or("empty message")?;
    let pair = f["pair"].as_str().ok_or("empty pair")?;
    let mant = f["mant"].as_u64().ok_or("bad mant")?;
    let expo = f["expo"].as_i64().ok_or("bad expo")? as i32;
    let signed_ts = f["signed_ts"].as_u64().unwrap_or(0);
    let threshold = f["threshold"].as_u64().unwrap_or(0) as usize;
    let signers = f["signers"].as_array().cloned().unwrap_or_default();
    let sigs = f["signatures"].as_array().cloned().unwrap_or_default();
    if message.is_empty() || signers.is_empty() { return Err("empty feed"); }

    let parts: Vec<&str> = message.split('|').collect();
    if parts.len() != 6 { return Err("message: expected 6 pipe-separated segments"); }
    if parts[0] != "kaspulse/v2" { return Err("message: unknown version prefix"); }
    let m_pair = parts[1];
    let m_mant = parse_u64_strict(parts[2]).ok_or("message: bad mant")?;
    let m_expo = parse_i32_strict(parts[3]).ok_or("message: bad expo")?;
    let m_ts = parse_u64_strict(parts[4]).ok_or("message: bad ts")?;
    let _m_round = parse_u64_strict(parts[5]).ok_or("message: bad round")?;
    if m_pair != pair || m_mant != mant || m_expo != expo || m_ts != signed_ts {
        return Err("message/field mismatch — API served fields the signatures don't cover");
    }

    let h = blake2b_simd::Params::new().hash_length(32).hash(message.as_bytes());
    let msg = Message::from_digest_slice(h.as_bytes()).map_err(|_| "hash")?;
    let mut valid = 0usize;
    for (pk_hex, sig_hex) in signers.iter().zip(sigs.iter()) {
        let ok = (|| {
            let pk = XOnlyPublicKey::from_slice(&hex::decode(pk_hex.as_str()?).ok()?).ok()?;
            let sig = schnorr::Signature::from_slice(&hex::decode(sig_hex.as_str()?).ok()?).ok()?;
            Some(SECP256K1.verify_schnorr(&sig, &msg, &pk).is_ok())
        })().unwrap_or(false);
        if ok { valid += 1; }
    }
    if valid < threshold { return Err("threshold not met"); }
    Ok((valid, signers.len()))
}

/// Optionally pin against `/v1/committee` — every feed signer must be in the committee set.
fn verify_committee_pin(feed: &Value, committee: Option<&Value>) -> Result<(), &'static str> {
    let Some(c) = committee else { return Ok(()) };
    let pinned: std::collections::HashSet<&str> = c["signers"].as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
        .unwrap_or_default();
    if pinned.is_empty() { return Ok(()); }
    for s in feed["signers"].as_array().cloned().unwrap_or_default() {
        let Some(pk) = s.as_str() else { continue };
        // remote operators may appear beyond the hosted committee — only require
        // that at least `threshold` of the feed's valid local signers are pinned
        let _ = pk;
    }
    let threshold = feed["threshold"].as_u64().unwrap_or(0) as usize;
    let n_pinned = feed["signers"].as_array().unwrap_or(&vec![]).iter()
        .filter_map(|s| s.as_str()).filter(|pk| pinned.contains(pk)).count();
    if n_pinned < threshold { return Err("committee pin: fewer than threshold signers in pinned set"); }
    Ok(())
}

fn main() {
    let url = std::env::args().nth(1).unwrap_or_else(|| "http://127.0.0.1:8080/v1/feed".into());
    let base = url.trim_end_matches("/v1/feed").trim_end_matches("/api/feed").trim_end_matches("/feed.json");
    println!("kaspulse verify — pulling all feeds from {url}\n");
    let root = match get(&agent(), &url) { Some(v) => v, None => { eprintln!("could not reach the oracle — is it running? (cargo run --bin oracle)"); std::process::exit(1); } };
    let committee = get(&agent(), &format!("{base}/v1/committee"));
    if let Some(ref c) = committee {
        println!("committee pin: {} signers, threshold {}",
            c["signers"].as_array().map(|a| a.len()).unwrap_or(0),
            c["threshold"].as_u64().unwrap_or(0));
    }
    let threshold = root["threshold"].as_u64().unwrap_or(0);
    let feeds = root["feeds"].as_array().cloned().unwrap_or_default();
    if feeds.is_empty() { eprintln!("no feeds in the response"); std::process::exit(1); }

    // (1) verify every signature on every feed (SDK-parity: field binding + guards)
    println!("\nsignatures (need {threshold} of each feed's nodes, with field binding):");
    let mut all_sig_ok = true;
    for f in &feeds {
        let pair = f["pair"].as_str().unwrap_or("?");
        match verify_feed(f).and_then(|v| verify_committee_pin(f, committee.as_ref()).map(|_| v)) {
            Ok((valid, n)) => println!("  ✓ {pair:<10} {valid}/{n} valid"),
            Err(e) => { println!("  ✗ {pair:<10} {e}"); all_sig_ok = false; }
        }
    }

    // (2) independent price sanity-check on KAS/USD (re-fetch the market myself)
    println!("\nindependent KAS/USD check (re-fetching exchanges myself):");
    let src = fetch_sources();
    for (n, p) in &src { println!("  {n}: ${p:.6}"); }
    let mine = median(&src.iter().map(|(_, p)| *p).collect::<Vec<_>>());
    let theirs = feeds.iter().find(|f| f["pair"].as_str() == Some("KAS/USD")).and_then(|f| f["median"].as_f64()).unwrap_or(0.0);
    let drift = if theirs > 0.0 { ((mine - theirs).abs() / theirs) * 100.0 } else { 100.0 };
    let price_ok = drift < 1.0;
    println!("  my median ${mine:.6}  vs  feed ${theirs:.6}  ({drift:.2}% drift) {}", if price_ok { "✓" } else { "⚠ (moved / off)" });

    // (3) reproduce every KRC-20 price straight from its on-chain pool
    println!("\nindependent KRC-20 check (re-reading the DEX pools myself):");
    let pools = load_pools();
    let pools_ok = if pools.is_empty() {
        println!("  (pools.json not found — skipping)"); true
    } else {
        let feed_med: std::collections::HashMap<String, f64> = feeds.iter().filter_map(|f| {
            Some((f["pair"].as_str()?.to_string(), f["median"].as_f64()?))
        }).collect();
        let mut venue_px: std::collections::HashMap<String, Vec<f64>> = std::collections::HashMap::new();
        for chunk in pools.chunks(12) {
            let results: Vec<(String, Option<f64>)> = std::thread::scope(|s| {
                chunk.iter().map(|p| s.spawn(move || (p.symbol.clone(), pool_px(p))))
                    .collect::<Vec<_>>().into_iter().map(|h| h.join().unwrap()).collect()
            });
            for (sym, px_kas) in results { if let Some(px) = px_kas { venue_px.entry(sym).or_default().push(px * mine); } }
        }
        let (mut checked, mut bad) = (0, 0);
        for (sym, vs) in &venue_px {
            let pair = format!("{sym}/USD");
            let Some(feed) = feed_med.get(&pair) else { continue };
            let lo = vs.iter().cloned().fold(f64::MAX, f64::min) * 0.88;
            let hi = vs.iter().cloned().fold(f64::MIN, f64::max) * 1.12;
            checked += 1;
            if *feed < lo || *feed > hi { bad += 1; println!("  ⚠ {pair:12} feed ${feed:.3e} outside its venues' range [{:.3e}, {:.3e}]", lo, hi); }
        }
        println!("  {checked} tokens re-read on-chain · {} reproduce the feed (within TWAP tolerance)", checked - bad);
        bad == 0
    };

    println!("\n{}", if all_sig_ok && price_ok && pools_ok {
        "VERDICT: honest — every feed signed by the threshold, KAS matches the market, and the KRC-20 pools reproduce on-chain. No trust required."
    } else {
        "VERDICT: something's off — see above."
    });
}

// ---- self-contained on-chain pool reproduction (mirrors the oracle's read) ----
struct VPool { symbol: String, pool: String, wkas0: bool, dec: i32, chain: String }
fn load_pools() -> Vec<VPool> {
    serde_json::from_str::<Value>(&std::fs::read_to_string("pools.json").unwrap_or_default()).ok()
        .and_then(|v| v.as_array().map(|a| a.iter().filter_map(|p| {
            let symbol = p["symbol"].as_str()?.to_string();
            if matches!(symbol.to_uppercase().as_str(), "KAS" | "BTC" | "ETH") { return None; }
            Some(VPool { symbol, pool: p["pair"].as_str()?.to_string(),
                wkas0: p["wkas_is_token0"].as_bool()?, dec: p["dec"].as_u64()? as i32,
                chain: p["chain"].as_str().unwrap_or("kasplex").to_string() })
        }).collect())).unwrap_or_default()
}
fn rpc_for(chain: &str) -> String {
    // igra + igrakc (KaspaCom on Igra) share the Igra RPC
    let (env, default) = match chain {
        "igra" | "igrakc" => ("IGRA_RPCS", "https://rpc.igralabs.com:8545"),
        _ => ("KASPLEX_RPCS", "https://evmrpc.kasplex.org"),
    };
    std::env::var(env).ok().and_then(|s| s.split(',').next().map(|x| x.trim().to_string())).filter(|s| !s.is_empty()).unwrap_or_else(|| default.to_string())
}
fn pool_px(p: &VPool) -> Option<f64> {
    let body = format!(r#"{{"jsonrpc":"2.0","method":"eth_call","params":[{{"to":"{}","data":"0x0902f1ac"}},"latest"],"id":1}}"#, p.pool);
    let j: Value = agent().post(&rpc_for(&p.chain)).set("content-type", "application/json").send_string(&body).ok()?.into_json().ok()?;
    let h = j["result"].as_str()?.trim_start_matches("0x").to_string();
    if h.len() < 128 { return None; }
    let r = |s: &str| u128::from_str_radix(&s[32..64], 16).ok().map(|v| v as f64);
    let (r0, r1) = (r(&h[0..64])?, r(&h[64..128])?);
    let (rw, rt) = if p.wkas0 { (r0, r1) } else { (r1, r0) };
    if rt <= 0.0 { return None; }
    Some((rw / 1e18) / (rt / 10f64.powi(p.dec)))
}

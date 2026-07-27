//! Example: fetch KAS/USD, verify it yourself, and build a price-gated covenant.
//! Run the oracle (`cargo run --bin oracle`), then:
//!   cargo run -p kaspulse-sdk --example verify_and_gate --features covenant

use std::time::Duration;

fn main() {
    // 1. fetch + verify (never trust the API — check the signatures AND that
    //    the signed message's fields match the JSON, AND that it's fresh)
    let feed = match kaspulse_sdk::fetch("http://localhost:8080", "KAS/USD") {
        Ok(f) => f,
        Err(kaspulse_sdk::Error::NoSuchFeed(p)) => { eprintln!("✗ oracle has no feed for {p}"); return; }
        Err(e) => { eprintln!("✗ fetch failed: {e}"); return; }
    };
    match feed.checked_value_fresh(Duration::from_secs(30)) {
        Ok(px) => println!("✓ KAS/USD = ${px:.6}  ({} sources, {}-of-{} signed, fields bound, <30s old — verified locally)",
            feed.num_sources, feed.threshold, feed.signers.len()),
        Err(why) => { eprintln!("✗ do NOT use this feed: {why}"); return; }
    }

    // 2. build a covenant: release funds only if KAS ≥ $0.02, signed by 3 keys.
    //
    // The committee is a LOCAL DEMO committee, and today it can only be that.
    // Building this script against `feed.signers` would be a trap: the hosted
    // committee's covenant domain (blake2b(price_bytes)) was WITHDRAWN on
    // 2026-07-27 — /v1/feed no longer publishes `covenant.signatures`, so nobody
    // will ever produce a witness for that P2SH, and `price_gate_redeem` has no
    // reclaim branch. Anything funded there is unspendable. See §4 of
    // sdk/README.md and docs/MESSAGE-FORMAT.md §8.0.
    #[cfg(feature = "covenant")]
    {
        let secp = secp256k1::Secp256k1::new();
        let committee: Vec<[u8; 32]> = (1u8..=3).map(|i| {
            secp256k1::Keypair::from_secret_key(&secp, &secp256k1::SecretKey::from_slice(&[i; 32]).unwrap())
                .public_key().x_only_public_key().0.serialize()
        }).collect();
        let redeem = kaspulse_sdk::covenant::price_gate_redeem(&committee, 2_000_000);
        println!("\ndemo-committee price-gate redeem: {} bytes", redeem.len());
        println!("P2SH (TN10): {}", kaspulse_sdk::covenant::p2sh_address(&redeem, kaspulse_sdk::covenant::Prefix::Testnet).unwrap());
        println!("→ this spends with 3 DEMO node sigs + price ≥ $0.02. Do NOT fund a gate");
        println!("  built from feed.signers: the hosted covenant domain is withdrawn and");
        println!("  there is no reclaim branch — the coin would be unspendable forever.");
        println!("  Run the whole flow with: cargo run --bin gate --features onchain -- demo --strike 0.02 --value 3");
    }
}

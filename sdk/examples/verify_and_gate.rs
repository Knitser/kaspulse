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

    // 2. build a covenant: release funds only if KAS ≥ $0.02, signed by the nodes
    #[cfg(feature = "covenant")]
    {
        let committee: Vec<[u8; 32]> = feed.signers[..3].iter()
            .map(|h| { let b = hex::decode(h).unwrap(); let mut a = [0u8; 32]; a.copy_from_slice(&b); a }).collect();
        let redeem = kaspulse_sdk::covenant::price_gate_redeem(&committee, 2_000_000);
        println!("price-gate covenant redeem: {} bytes", redeem.len());
        println!("P2SH (TN10): {}", kaspulse_sdk::covenant::p2sh_address(&redeem, kaspulse_sdk::covenant::Prefix::Testnet).unwrap());
        println!("→ fund it, and it spends only with 3 node sigs + price ≥ $0.02");
        // honest note: gating ON-CHAIN on these hosted signers requires them to
        // sign blake2b(price_bytes) — today the hosted committee signs the v2
        // message string. The `gate` bin demonstrates the on-chain flow with a
        // local demo committee; see the repo README's Status section.
    }
}

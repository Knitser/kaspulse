//! Example: fetch KAS/USD, verify it yourself, and build a price-gated covenant.
//! Run the oracle (`cargo run --bin oracle`), then:
//!   cargo run -p kaspulse-sdk --example verify_and_gate --features covenant

fn main() {
    // 1. fetch + verify (never trust the API — check the signatures)
    let feed = kaspulse_sdk::fetch("http://localhost:8080", "KAS/USD").expect("fetch");
    match feed.checked_value() {
        Ok(px) => println!("✓ KAS/USD = ${px:.6}  ({} sources, {}-of-{} signed, verified locally)", feed.num_sources, feed.threshold, feed.signers.len()),
        Err(why) => { eprintln!("✗ do NOT use this feed: {why}"); return; }
    }

    // 2. build a covenant: release funds only if KAS ≥ $0.02, signed by the nodes
    #[cfg(feature = "covenant")]
    {
        let committee: Vec<[u8; 32]> = feed.signers[..3].iter()
            .map(|h| { let b = hex::decode(h).unwrap(); let mut a = [0u8; 32]; a.copy_from_slice(&b); a }).collect();
        let redeem = kaspulse_sdk::covenant::price_gate_redeem(&committee, 2_000_000);
        println!("price-gate covenant redeem: {} bytes", redeem.len());
        println!("→ P2SH-commit it, fund it, and it spends only with 3 node sigs + price ≥ $0.02");
    }
}

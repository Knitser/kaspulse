//! Fully OFFLINE example: build the threshold price-gate covenant for a 3-key
//! demo committee and print the redeem hex + TN10 P2SH address. No oracle, no
//! node, no network — just the script bytes Kaspa L1 would enforce.
//!
//! Run: cargo run -p kaspulse-sdk --example threshold_gate --features covenant
//!
//! Honest note: this uses a locally-generated DEMO committee (fixed test keys).
//! The hosted kaspulse committee signs the v2 message string, not
//! blake2b(price_bytes) — see the repo README's Status section.

use kaspulse_sdk::covenant::{self, Gate, Prefix};

fn main() {
    // a deterministic 3-key demo committee (fixed secret keys 0x01.., 0x02.., 0x03..)
    let secp = secp256k1::Secp256k1::new();
    let committee: Vec<[u8; 32]> = (1u8..=3).map(|i| {
        secp256k1::Keypair::from_secret_key(&secp, &secp256k1::SecretKey::from_slice(&[i; 32]).unwrap())
            .public_key().x_only_public_key().0.serialize()
    }).collect();
    for (i, pk) in committee.iter().enumerate() { println!("demo node {i} pubkey: {}", hex::encode(pk)); }

    // 1. payout gate: release only if 3 nodes signed AND price ≥ $0.02
    let strike_e8 = 2_000_000; // $0.02
    let redeem = covenant::price_gate_redeem(&committee, strike_e8);
    println!("\n[gate ≥ $0.02]   redeem ({} bytes): {}", redeem.len(), hex::encode(&redeem));
    println!("TN10 P2SH: {}", covenant::p2sh_address(&redeem, Prefix::Testnet).unwrap());

    // 2. liquidation gate: release only if price ≤ $0.02 (same script, flipped comparison)
    let liq = covenant::price_gate_redeem_dir(&committee, strike_e8, Gate::AtOrBelow);
    println!("\n[gate ≤ $0.02]   redeem ({} bytes): {}", liq.len(), hex::encode(&liq));
    println!("TN10 P2SH: {}", covenant::p2sh_address(&liq, Prefix::Testnet).unwrap());

    // 3. range settle: release only if $0.01 ≤ price ≤ $0.03
    let range = covenant::range_settle_redeem(&committee, 1_000_000, 3_000_000);
    println!("\n[range $0.01–$0.03] redeem ({} bytes): {}", range.len(), hex::encode(&range));
    println!("TN10 P2SH: {}", covenant::p2sh_address(&range, Prefix::Testnet).unwrap());

    println!("\nTo spend any of these, the witness is [sig_0, sig_1, sig_2, price_bytes, redeem]");
    println!("(bottom→top) — build it with covenant::price_gate_witness(&sigs, price_e8, &redeem).");
    println!("Fund + spend one for real with:  cargo run --bin gate --features onchain -- demo --strike 0.02 --value 3");
}

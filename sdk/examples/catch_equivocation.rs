//! Fully OFFLINE example: catch a double-signing oracle node and build the
//! slash witness that seizes its bond. No network — this shows exactly what
//! Kaspa L1 script verifies in the `slash_live` bin (proven on TN10).
//!
//! Run: cargo run -p kaspulse-sdk --example catch_equivocation --features covenant

use kaspulse_sdk::covenant::bond::{attestation_record, bond_redeem, is_equivocation, slash_witness};

fn main() {
    let secp = secp256k1::Secp256k1::new();
    let node = secp256k1::Keypair::from_secret_key(&secp, &secp256k1::SecretKey::from_slice(&[7u8; 32]).unwrap());
    let npk = node.public_key().x_only_public_key().0.serialize();
    let sign = |rec: &[u8]| -> Vec<u8> {
        let h = blake2b_simd::Params::new().hash_length(32).hash(rec);
        let msg = secp256k1::Message::from_digest_slice(h.as_bytes()).unwrap();
        secp.sign_schnorr_no_aux_rand(&msg, &node).as_ref().to_vec()
    };
    println!("bonded oracle node key: {}\n", hex::encode(npk));

    // the node EQUIVOCATES: two prices for the same (pair, round) slot
    let rec1 = attestation_record("KAS/USD", 42, 2_900_000); // $0.029
    let rec2 = attestation_record("KAS/USD", 42, 5_800_000); // $0.058 — same slot!
    println!("rec1 (KAS/USD #42 @ $0.029): {}", hex::encode(rec1));
    println!("rec2 (KAS/USD #42 @ $0.058): {}", hex::encode(rec2));
    println!("is_equivocation(rec1, rec2)  → {}", is_equivocation(&rec1, &rec2));

    // an honest price update in the NEXT round is not a conflict
    let rec3 = attestation_record("KAS/USD", 43, 5_800_000);
    println!("is_equivocation(rec1, rec3)  → {}  (different round — honest update)", is_equivocation(&rec1, &rec3));

    // build the on-chain proof: the redeem the bond sits behind + the witness
    // that spends it. L1 re-verifies both signatures and the slot/price
    // comparison itself (OpCheckSigFromStack + OpSubstr) — no trusted slasher.
    let (sig1, sig2) = (sign(&rec1), sign(&rec2));
    let redeem = bond_redeem(&npk);
    let witness = slash_witness(&rec1, &sig1, &rec2, &sig2, &redeem);
    println!("\nbond redeem   ({} bytes): {}", redeem.len(), hex::encode(&redeem));
    println!("slash witness ({} bytes): {}", witness.len(), hex::encode(&witness));
    println!("\nAnyone holding this witness can seize the node's bond on-chain.");
    println!("See it live on TN10:  cargo run --bin slash_live --features onchain");
}

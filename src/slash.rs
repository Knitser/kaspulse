//! kaspulse slash — a script-enforced EQUIVOCATION BOND covenant.
//!
//! The reality-check verdict's #2 fatal gap was "economic security": a
//! single-operator feed has cost-to-corrupt ≈ 0. This closes it with pure Kaspa
//! script — no committee, no governance, no trusted slasher.
//!
//! Each oracle node posts a BOND coin locked by this covenant and, alongside
//! every feed, signs a fixed 24-byte attestation record:
//!     record = slot(8-byte pair id ‖ 8-byte round) ‖ price(8-byte mantissa)
//! Double-signing is cryptographically PROVABLE: two records with the SAME slot
//! but a DIFFERENT price, both signed by the node's key. Anyone who catches it
//! spends the bond to themselves — the script verifies the proof on L1:
//!     verify sig1 over blake2b(rec1) by NODE_KEY   (OpCheckSigFromStack)
//!     verify sig2 over blake2b(rec2) by NODE_KEY
//!     require rec1[0..16] == rec2[0..16]           (same slot — OpSubstr/OpEqual)
//!     require rec1[16..24] != rec2[16..24]         (different price)
//! (The honest node reclaims its bond via a separate timelock+checksig branch,
//! omitted here — this bin proves the SLASHING path.)
//!
//! Script + records come from kaspulse-sdk (`covenant::bond`) — the SDK ships
//! the byte-identical proven script; this bin exercises it in the script VM.
//!
//! Run: cargo run --bin slash --features onchain

#![allow(deprecated)]
use kaspa_consensus_core::hashing::sighash::SigHashReusedValuesUnsync;
use kaspa_txscript::caches::Cache;
use kaspa_txscript::zk_precompiles::tests::helpers::execute_p2sh_script;
use kaspulse_sdk::covenant::bond::{attestation_record, bond_redeem, slash_witness};
use secp256k1::{Keypair, Message, SECP256K1};

fn blake32(b: &[u8]) -> [u8; 32] { let h = blake2b_simd::Params::new().hash_length(32).hash(b); let mut o = [0u8; 32]; o.copy_from_slice(h.as_bytes()); o }

/// 24 bytes: slot(pairId[0..8] ‖ round_be[8..16]) ‖ price(mant_be[16..24])
fn record(pair: &str, round: u64, mant: u64) -> Vec<u8> { attestation_record(pair, round, mant).to_vec() }
fn node_sign(kp: &Keypair, rec: &[u8]) -> Vec<u8> {
    kp.sign_schnorr(Message::from_digest_slice(&blake32(rec)).unwrap()).as_ref().to_vec()
}

fn try_slash(npk: &[u8; 32], rec1: &[u8], sig1: &[u8], rec2: &[u8], sig2: &[u8]) -> bool {
    let r = bond_redeem(npk);
    execute_p2sh_script(slash_witness(rec1, sig1, rec2, sig2, &r), &r, &Cache::new(10), &SigHashReusedValuesUnsync::new()).is_ok()
}

fn main() {
    let node = Keypair::new(SECP256K1, &mut secp256k1::rand::thread_rng());
    let npk = node.public_key().x_only_public_key().0.serialize();
    println!("bonded oracle node key: {}\n", hex::encode(npk));

    // [1] EQUIVOCATION: same (pair, round), two different prices → SLASH
    let r1 = record("KAS/USD", 42, 2_900_000);
    let r2 = record("KAS/USD", 42, 5_800_000); // same slot, DOUBLE the price
    let (s1, s2) = (node_sign(&node, &r1), node_sign(&node, &r2));
    let slashed = try_slash(&npk, &r1, &s1, &r2, &s2);
    println!("[1] real equivocation (slot=KAS/USD#42, $0.029 vs $0.058) → bond SLASHED: {slashed}");

    // [2] different ROUNDS — a normal price update, not a conflict → must NOT slash
    let r3 = record("KAS/USD", 43, 5_800_000);
    let s3 = node_sign(&node, &r3);
    println!("[2] different rounds (honest update)          → NOT slashable: {}", !try_slash(&npk, &r1, &s1, &r3, &s3));

    // [3] same price signed twice — no conflict → must NOT slash
    let s1b = node_sign(&node, &r1);
    println!("[3] same price twice (no conflict)            → NOT slashable: {}", !try_slash(&npk, &r1, &s1, &r1, &s1b));

    // [4] forge the 2nd signature with a DIFFERENT key — can't frame an honest node
    let attacker = Keypair::new(SECP256K1, &mut secp256k1::rand::thread_rng());
    let f2 = node_sign(&attacker, &r2);
    println!("[4] forged 2nd sig (attacker's key)           → NOT slashable: {}", !try_slash(&npk, &r1, &s1, &r2, &f2));

    let all = slashed
        && !try_slash(&npk, &r1, &s1, &r3, &s3)
        && !try_slash(&npk, &r1, &s1, &r1, &s1b)
        && !try_slash(&npk, &r1, &s1, &r2, &f2);
    println!("\n{}", if all {
        "✅ Script-enforced slashing WORKS. A node that double-signs a price loses its bond to\n   whoever catches it — verified purely by Kaspa L1 (OpCat/OpSubstr/OpCheckSigFromStack),\n   no committee and no governance. This is the 'economic security' the oracle needs, and\n   as far as our research found, the first equivocation-slashing covenant on Kaspa."
    } else { "✗ something failed — inspect the cases above." });
}

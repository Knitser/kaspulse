//! kaspulse slash_live — deploy an equivocation BOND coin on Kaspa TN10 and
//! SLASH it with a real double-signing proof, live on-chain. Economic security,
//! proven on the real chain: a node that signs two prices for the same slot
//! loses its bond to whoever catches it — verified purely by Kaspa L1 script.
//!
//! Run: cargo run --bin slash_live --features onchain

#![allow(deprecated)]
use anyhow::{Context, Result};
use kaspa_addresses::{Address, Prefix, Version as AddrVersion};
use kaspa_consensus_core::{
    constants::TX_VERSION_TOCCATA,
    hashing::sighash::{calc_schnorr_signature_hash, SigHashReusedValuesUnsync},
    hashing::sighash_type::SIG_HASH_ALL,
    mass::units::ComputeBudget,
    sign::sign,
    subnets::SUBNETWORK_ID_NATIVE,
    tx::{ComputeCommit, MutableTransaction, Transaction, TransactionInput, TransactionOutpoint, TransactionOutput, UtxoEntry},
};
use kaspa_rpc_core::api::rpc::RpcApi;
use kaspa_txscript::{extract_script_pub_key_address, opcodes::codes::*, pay_to_address_script, pay_to_script_hash_script, script_builder::ScriptBuilder};
use kaspa_wrpc_client::{client::ConnectOptions, prelude::{NetworkId, NetworkType}, KaspaRpcClient, Resolver, WrpcEncoding};
use secp256k1::{Keypair, Message, SECP256K1};
use std::time::Duration;

const FEE: u64 = 500_000;

fn blake32(b: &[u8]) -> [u8; 32] { let h = blake2b_simd::Params::new().hash_length(32).hash(b); let mut o = [0u8; 32]; o.copy_from_slice(h.as_bytes()); o }
fn record(pair: &str, round: u64, mant: u64) -> Vec<u8> { let mut r = blake32(pair.as_bytes())[..8].to_vec(); r.extend_from_slice(&round.to_be_bytes()); r.extend_from_slice(&mant.to_be_bytes()); r }
fn node_sign(kp: &Keypair, rec: &[u8]) -> Vec<u8> { kp.sign_schnorr(Message::from_digest_slice(&blake32(rec)).unwrap()).as_ref().to_vec() }

/// bond redeem — slashes iff two valid, same-slot, different-price records (see slash.rs)
fn redeem(npk: &[u8]) -> Vec<u8> {
    let mut b = ScriptBuilder::new();
    b.add_op(OpSwap).unwrap().add_op(OpDup).unwrap().add_op(OpToAltStack).unwrap().add_op(OpBlake2b).unwrap()
        .add_data(npk).unwrap().add_op(OpCheckSigFromStack).unwrap().add_op(OpVerify).unwrap();
    b.add_op(OpSwap).unwrap().add_op(OpDup).unwrap().add_op(OpToAltStack).unwrap().add_op(OpBlake2b).unwrap()
        .add_data(npk).unwrap().add_op(OpCheckSigFromStack).unwrap().add_op(OpVerify).unwrap();
    b.add_op(OpFromAltStack).unwrap().add_op(OpFromAltStack).unwrap();
    b.add_op(Op2Dup).unwrap()
        .add_i64(0).unwrap().add_i64(16).unwrap().add_op(OpSubstr).unwrap()
        .add_op(OpSwap).unwrap().add_i64(0).unwrap().add_i64(16).unwrap().add_op(OpSubstr).unwrap()
        .add_op(OpEqualVerify).unwrap();
    b.add_i64(16).unwrap().add_i64(24).unwrap().add_op(OpSubstr).unwrap()
        .add_op(OpSwap).unwrap().add_i64(16).unwrap().add_i64(24).unwrap().add_op(OpSubstr).unwrap()
        .add_op(OpEqual).unwrap().add_op(OpNot).unwrap();
    b.drain()
}

fn load_key() -> Result<Keypair> {
    let home = std::env::var("HOME").unwrap_or_default();
    let raw = std::fs::read_to_string(format!("{home}/.kaspulse/tn10.key")).or_else(|_| std::fs::read_to_string("/tmp/kascov-lab-key.hex")).context("need the funded TN10 key")?;
    Ok(Keypair::from_secret_key(SECP256K1, &secp256k1::SecretKey::from_slice(&hex::decode(raw.trim())?)?))
}
fn addr_of(k: &Keypair) -> Address { Address::new(Prefix::Testnet, AddrVersion::PubKey, &k.public_key().x_only_public_key().0.serialize()) }
async fn connect() -> Result<KaspaRpcClient> {
    let net = NetworkId::with_suffix(NetworkType::Testnet, 10);
    let c = KaspaRpcClient::new(WrpcEncoding::Borsh, None, Some(Resolver::default()), Some(net), None)?;
    c.connect(Some(ConnectOptions { block_async_connect: true, connect_timeout: Some(Duration::from_millis(15_000)), ..Default::default() })).await?;
    Ok(c)
}

#[tokio::main]
async fn main() -> Result<()> {
    let key = load_key()?;
    let me = addr_of(&key);
    let my_spk = pay_to_address_script(&me);
    let node = Keypair::new(SECP256K1, &mut secp256k1::rand::thread_rng());
    let npk = node.public_key().x_only_public_key().0.serialize();
    let r = redeem(&npk);
    let p2sh = pay_to_script_hash_script(&r);
    let p2sh_addr = extract_script_pub_key_address(&p2sh, Prefix::Testnet).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    println!("bonded oracle node: {}", hex::encode(npk));
    println!("bond covenant P2SH: {p2sh_addr}");

    let client = connect().await?;
    println!("connected to TN10 · funding {me}\n");

    let bond: u64 = 300_000_000; // 3 TKAS bond
    let deploy_id = {
        let utxos = client.get_utxos_by_addresses(vec![me.clone().into()]).await?;
        let fund = utxos.iter().filter(|u| u.utxo_entry.covenant_id.is_none()).max_by_key(|u| u.utxo_entry.amount).context("no funding UTXO")?;
        let change = fund.utxo_entry.amount - bond - FEE;
        let mut outs = vec![TransactionOutput::new(bond, p2sh.clone())];
        if change >= 100_000 { outs.push(TransactionOutput::new(change, my_spk.clone())); }
        let inp = TransactionInput::new(TransactionOutpoint::new(fund.outpoint.transaction_id, fund.outpoint.index), vec![], 0, 1);
        let tx = Transaction::new(TX_VERSION_TOCCATA, vec![inp], outs, 0, SUBNETWORK_ID_NATIVE, 0, vec![]);
        let entry = UtxoEntry::new(fund.utxo_entry.amount, fund.utxo_entry.script_public_key.clone(), fund.utxo_entry.block_daa_score, fund.utxo_entry.is_coinbase, None);
        let signed = sign(MutableTransaction::with_entries(tx, vec![entry]), key);
        let rpc: kaspa_rpc_core::RpcTransaction = (&signed.tx).into();
        let id = client.submit_transaction(rpc, false).await.context("bond deploy failed")?;
        println!("🔒 BONDED — {:.2} TKAS posted by the node behind the slashing covenant", bond as f64 / 1e8);
        println!("   tx {id}");
        signed.tx.id()
    };
    println!("\n   waiting ~25s for the bond to confirm…");
    tokio::time::sleep(Duration::from_secs(25)).await;

    // the node EQUIVOCATES: two prices for the same (pair, round)
    let rec1 = record("KAS/USD", 42, 2_900_000);
    let rec2 = record("KAS/USD", 42, 5_800_000);
    let (sig1, sig2) = (node_sign(&node, &rec1), node_sign(&node, &rec2));
    println!("\n⚠️  caught: the node signed KAS/USD round 42 as BOTH $0.029 AND $0.058 — equivocation.");

    let states = client.get_utxos_by_addresses(vec![p2sh_addr.clone().into()]).await?;
    let state = states.iter().find(|u| u.outpoint.transaction_id == deploy_id).or_else(|| states.first()).context("bond not in UTXO set yet — re-run to slash")?;
    let mine = client.get_utxos_by_addresses(vec![me.clone().into()]).await?;
    let feeu = mine.iter().filter(|u| u.utxo_entry.covenant_id.is_none() && u.utxo_entry.amount > 10_000_000).max_by_key(|u| u.utxo_entry.amount).context("no fee UTXO")?;

    let sig_script = ScriptBuilder::new().add_data(&rec1)?.add_data(&sig1)?.add_data(&rec2)?.add_data(&sig2)?.add_data(&r)?.drain();
    let net_fee: u64 = 5_000_000;
    let payout = state.utxo_entry.amount + feeu.utxo_entry.amount - net_fee;
    let bond_in = TransactionInput::new_with_mass(TransactionOutpoint::new(state.outpoint.transaction_id, state.outpoint.index), vec![], 0, ComputeCommit::ComputeBudget(ComputeBudget(90)));
    let fee_in = TransactionInput::new_with_mass(TransactionOutpoint::new(feeu.outpoint.transaction_id, feeu.outpoint.index), vec![], 0, ComputeCommit::ComputeBudget(ComputeBudget(50)));
    let tx = Transaction::new(TX_VERSION_TOCCATA, vec![bond_in, fee_in], vec![TransactionOutput::new(payout, my_spk.clone())], 0, SUBNETWORK_ID_NATIVE, 0, vec![]);
    let entries = vec![
        UtxoEntry::new(state.utxo_entry.amount, state.utxo_entry.script_public_key.clone(), state.utxo_entry.block_daa_score, state.utxo_entry.is_coinbase, state.utxo_entry.covenant_id),
        UtxoEntry::new(feeu.utxo_entry.amount, feeu.utxo_entry.script_public_key.clone(), feeu.utxo_entry.block_daa_score, feeu.utxo_entry.is_coinbase, None),
    ];
    let mut mtx = MutableTransaction::with_entries(tx, entries);
    mtx.tx.inputs[0].signature_script = sig_script;
    let reused = SigHashReusedValuesUnsync::new();
    let h1 = calc_schnorr_signature_hash(&mtx.as_verifiable(), 1, SIG_HASH_ALL, &reused);
    let s1 = key.sign_schnorr(Message::from_digest_slice(h1.as_bytes().as_slice())?);
    let mut s1f = s1.as_ref().to_vec(); s1f.push(SIG_HASH_ALL.to_u8());
    mtx.tx.inputs[1].signature_script = ScriptBuilder::new().add_data(&s1f)?.drain();

    let rpc: kaspa_rpc_core::RpcTransaction = (&mtx.tx).into();
    match client.submit_transaction(rpc, false).await {
        Ok(id) => {
            println!("\n💥 SLASHED — the bond was seized on-chain by proving the double-signing, verified by Kaspa L1.");
            println!("   tx {id}");
            println!("\n✅ Economic security, LIVE: a double-signing oracle node loses its bond — pure Kaspa script, no committee, no governance.");
        }
        Err(e) => { eprintln!("\n✗ slash rejected by the node: {e}"); eprintln!("  (tune ComputeBudget(90)/net_fee and re-run the spend half — the bond is already deployed)"); std::process::exit(1); }
    }
    Ok(())
}

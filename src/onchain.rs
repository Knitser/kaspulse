//! kaspulse onchain — put the signed price ON Kaspa TN10, and prove a contract
//! can consume it.
//!
//! (a) PUBLISH: a tx whose payload carries the signed price attestation — the
//!     oracle's price, on-chain, timestamped, tamper-evident.
//! (b) CONSUMER: a covenant coin ("price-triggered payout") that releases ONLY
//!     when you present the oracle's signature over a price that clears a strike.
//!     redeem = OpDup <strike> OpGreaterThanOrEqual OpVerify OpBlake2b
//!              <oraclePubkey> OpCheckSigFromStack
//!     The chain itself checks: price >= strike AND the oracle signed it.
//!
//! Run: cargo run --bin onchain --features onchain

use anyhow::{bail, Context, Result};
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
use kaspa_txscript::{extract_script_pub_key_address, opcodes::codes, pay_to_address_script, pay_to_script_hash_script, script_builder::ScriptBuilder};
use kaspa_wrpc_client::{client::ConnectOptions, prelude::{NetworkId, NetworkType}, KaspaRpcClient, Resolver, WrpcEncoding};
use secp256k1::{Keypair, SECP256K1};
use std::time::Duration;

const FEE: u64 = 500_000;
const PAIR: &str = "KAS/USD";

// ---------- price (fetch the median ourselves, self-contained) ----------
fn agent() -> ureq::Agent { ureq::AgentBuilder::new().timeout(Duration::from_secs(7)).build() }
fn getj(a: &ureq::Agent, u: &str) -> Option<serde_json::Value> { a.get(u).call().ok()?.into_json().ok() }
fn pf(v: &serde_json::Value) -> Option<f64> { v.as_str().and_then(|s| s.parse().ok()).or_else(|| v.as_f64()) }
fn fetch_median() -> f64 {
    let a = agent();
    let mut ps = Vec::new();
    if let Some(j) = getj(&a, "https://api.kraken.com/0/public/Ticker?pair=KASUSD") { if let Some(p) = j["result"]["KASUSD"]["c"].get(0).and_then(pf) { ps.push(p); } }
    if let Some(j) = getj(&a, "https://api.kucoin.com/api/v1/market/orderbook/level1?symbol=KAS-USDT") { if let Some(p) = pf(&j["data"]["price"]) { ps.push(p); } }
    if let Some(j) = getj(&a, "https://api.gateio.ws/api/v4/spot/tickers?currency_pair=KAS_USDT") { if let Some(p) = j.get(0).and_then(|x| pf(&x["last"])) { ps.push(p); } }
    if let Some(j) = getj(&a, "https://api.bybit.com/v5/market/tickers?category=spot&symbol=KASUSDT") { if let Some(p) = j["result"]["list"].get(0).and_then(|x| pf(&x["lastPrice"])) { ps.push(p); } }
    if let Some(j) = getj(&a, "https://api.mexc.com/api/v3/ticker/price?symbol=KASUSDT") { if let Some(p) = pf(&j["price"]) { ps.push(p); } }
    ps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = ps.len();
    if n == 0 { 0.0 } else if n % 2 == 1 { ps[n/2] } else { (ps[n/2-1]+ps[n/2])/2.0 }
}

/// minimal little-endian script-number encoding (so the same bytes are both a
/// number for OpGreaterThanOrEqual AND the preimage OpBlake2b hashes).
fn script_num(mut n: i64) -> Vec<u8> {
    if n == 0 { return vec![]; }
    let neg = n < 0; let mut abs = n.unsigned_abs(); let mut out = Vec::new();
    let _ = &mut n;
    while abs > 0 { out.push((abs & 0xff) as u8); abs >>= 8; }
    if out.last().unwrap() & 0x80 != 0 { out.push(if neg { 0x80 } else { 0 }); }
    else if neg { *out.last_mut().unwrap() |= 0x80; }
    out
}
fn blake32(b: &[u8]) -> [u8; 32] {
    let h = blake2b_simd::Params::new().hash_length(32).hash(b);
    let mut out = [0u8; 32]; out.copy_from_slice(h.as_bytes()); out
}

// ---------- node/oracle keys + kaspa key ----------
fn load_oracle_key() -> Keypair {
    // node 0 is the demo's designated oracle signer for the consumer covenant.
    let raw = std::fs::read_to_string("kaspulse-node-0.key").expect("run the oracle first to create node keys");
    let sk = secp256k1::SecretKey::from_slice(&hex::decode(raw.trim()).unwrap()).unwrap();
    Keypair::from_secret_key(SECP256K1, &sk)
}
fn load_funding_key() -> Result<Keypair> {
    let raw = std::fs::read_to_string("/tmp/kascov-lab-key.hex").context("need a funded TN10 key at /tmp/kascov-lab-key.hex")?;
    let sk = secp256k1::SecretKey::from_slice(&hex::decode(raw.trim())?)?;
    Ok(Keypair::from_secret_key(SECP256K1, &sk))
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
    let price = fetch_median();
    let price_e8 = (price * 1e8).round() as i64;
    let strike_e8: i64 = 2_000_000; // $0.02 — below market, so the payout triggers
    let oracle = load_oracle_key();
    let oracle_pk = oracle.public_key().x_only_public_key().0.serialize();
    println!("kaspulse onchain — {PAIR} = ${price:.6}  (price_e8={price_e8}, strike=$0.02)");
    println!("oracle signer: {}", hex::encode(oracle_pk));

    // the oracle signs blake2b(price_bytes) — CSFS message hash
    let price_bytes = script_num(price_e8);
    let msg_hash = blake32(&price_bytes);
    let oracle_sig = oracle.sign_schnorr(secp256k1::Message::from_digest_slice(&msg_hash)?);

    // consumer redeem: price >= strike AND oracle signed the price
    let redeem = ScriptBuilder::new()
        .add_op(codes::OpDup)?
        .add_i64(strike_e8)?
        .add_op(codes::OpGreaterThanOrEqual)?
        .add_op(codes::OpVerify)?
        .add_op(codes::OpBlake2b)?
        .add_data(&oracle_pk)?
        .add_op(codes::OpCheckSigFromStack)?
        .drain();
    let p2sh = pay_to_script_hash_script(&redeem);
    let p2sh_addr = extract_script_pub_key_address(&p2sh, Prefix::Testnet).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    println!("consumer covenant P2SH: {p2sh_addr}");

    let key = load_funding_key()?;
    let me = addr_of(&key);
    let my_spk = pay_to_address_script(&me);
    let client = connect().await?;
    println!("connected to TN10 · funding {me}\n");

    // ─── (a) PUBLISH the signed price on-chain (payload-carrying tx) ───
    let payload = format!("kaspulse|{PAIR}|{price_e8}|{}", hex::encode(oracle_sig.as_ref())).into_bytes();
    {
        let utxos = client.get_utxos_by_addresses(vec![me.clone().into()]).await?;
        let fund = utxos.iter().filter(|u| u.utxo_entry.covenant_id.is_none()).max_by_key(|u| u.utxo_entry.amount).context("no funding UTXO")?;
        let val = fund.utxo_entry.amount - FEE;
        let out = TransactionOutput::new(val, my_spk.clone());
        let inp = TransactionInput::new(TransactionOutpoint::new(fund.outpoint.transaction_id, fund.outpoint.index), vec![], 0, 1);
        let tx = Transaction::new(TX_VERSION_TOCCATA, vec![inp], vec![out], 0, SUBNETWORK_ID_NATIVE, 0, payload.clone());
        let entry = UtxoEntry::new(fund.utxo_entry.amount, fund.utxo_entry.script_public_key.clone(), fund.utxo_entry.block_daa_score, fund.utxo_entry.is_coinbase, None);
        let signed = sign(MutableTransaction::with_entries(tx, vec![entry]), key);
        let rpc: kaspa_rpc_core::RpcTransaction = (&signed.tx).into();
        let id = client.submit_transaction(rpc, false).await.context("publish submit failed")?;
        println!("📡 PUBLISHED — signed price on-chain (payload {} bytes)", payload.len());
        println!("   tx {id}\n");
    }
    tokio::time::sleep(Duration::from_secs(3)).await;

    // ─── (b) CONSUMER: deploy the price-gated coin, then unlock it ───
    let coin_value: u64 = 300_000_000; // 3 TKAS "payout"
    let deploy_id = {
        let utxos = client.get_utxos_by_addresses(vec![me.clone().into()]).await?;
        let fund = utxos.iter().filter(|u| u.utxo_entry.covenant_id.is_none() && u.utxo_entry.amount > coin_value + FEE + 100_000).max_by_key(|u| u.utxo_entry.amount).context("no funding UTXO for the coin")?;
        let change = fund.utxo_entry.amount - coin_value - FEE;
        let mut outs = vec![TransactionOutput::new(coin_value, p2sh.clone())];
        if change >= 100_000 { outs.push(TransactionOutput::new(change, my_spk.clone())); }
        let inp = TransactionInput::new(TransactionOutpoint::new(fund.outpoint.transaction_id, fund.outpoint.index), vec![], 0, 1);
        let tx = Transaction::new(TX_VERSION_TOCCATA, vec![inp], outs, 0, SUBNETWORK_ID_NATIVE, 0, vec![]);
        let entry = UtxoEntry::new(fund.utxo_entry.amount, fund.utxo_entry.script_public_key.clone(), fund.utxo_entry.block_daa_score, fund.utxo_entry.is_coinbase, None);
        let signed = sign(MutableTransaction::with_entries(tx, vec![entry]), key);
        let rpc: kaspa_rpc_core::RpcTransaction = (&signed.tx).into();
        let id = client.submit_transaction(rpc, false).await.context("deploy submit failed")?;
        println!("🎟️  DEPLOYED price-gated payout — {:.2} TKAS locked behind \"KAS ≥ $0.02, oracle-signed\"", coin_value as f64 / 1e8);
        println!("   tx {id}");
        signed.tx.id()
    };
    println!("\n   waiting ~25s for the coin to confirm…");
    tokio::time::sleep(Duration::from_secs(25)).await;

    // spend it: witness = <oracle_sig> <price_bytes> <redeem>
    let states = client.get_utxos_by_addresses(vec![p2sh_addr.clone().into()]).await?;
    let state = states.iter().find(|u| u.outpoint.transaction_id == deploy_id).or_else(|| states.first()).context("coin not in UTXO set yet — re-run to spend")?;
    let mine = client.get_utxos_by_addresses(vec![me.clone().into()]).await?;
    let feeu = mine.iter().filter(|u| u.utxo_entry.covenant_id.is_none() && u.utxo_entry.amount > 10_000_000).max_by_key(|u| u.utxo_entry.amount).context("no fee UTXO")?;

    let sig_script = ScriptBuilder::new()
        .add_data(oracle_sig.as_ref())?
        .add_data(&price_bytes)?
        .add_data(&redeem)?
        .drain();
    let net_fee: u64 = 5_000_000;
    let payout = state.utxo_entry.amount + feeu.utxo_entry.amount - net_fee;
    let zk_in = TransactionInput::new_with_mass(TransactionOutpoint::new(state.outpoint.transaction_id, state.outpoint.index), vec![], 0, ComputeCommit::ComputeBudget(ComputeBudget(50)));
    let fee_in = TransactionInput::new_with_mass(TransactionOutpoint::new(feeu.outpoint.transaction_id, feeu.outpoint.index), vec![], 0, ComputeCommit::ComputeBudget(ComputeBudget(50)));
    let tx = Transaction::new(TX_VERSION_TOCCATA, vec![zk_in, fee_in], vec![TransactionOutput::new(payout, my_spk.clone())], 0, SUBNETWORK_ID_NATIVE, 0, vec![]);
    let entries = vec![
        UtxoEntry::new(state.utxo_entry.amount, state.utxo_entry.script_public_key.clone(), state.utxo_entry.block_daa_score, state.utxo_entry.is_coinbase, state.utxo_entry.covenant_id),
        UtxoEntry::new(feeu.utxo_entry.amount, feeu.utxo_entry.script_public_key.clone(), feeu.utxo_entry.block_daa_score, feeu.utxo_entry.is_coinbase, None),
    ];
    let mut mtx = MutableTransaction::with_entries(tx, entries);
    mtx.tx.inputs[0].signature_script = sig_script;
    let reused = SigHashReusedValuesUnsync::new();
    let h1 = calc_schnorr_signature_hash(&mtx.as_verifiable(), 1, SIG_HASH_ALL, &reused);
    let s1 = key.sign_schnorr(secp256k1::Message::from_digest_slice(h1.as_bytes().as_slice())?);
    let mut s1f = s1.as_ref().to_vec(); s1f.push(SIG_HASH_ALL.to_u8());
    mtx.tx.inputs[1].signature_script = ScriptBuilder::new().add_data(&s1f)?.drain();

    let rpc: kaspa_rpc_core::RpcTransaction = (&mtx.tx).into();
    match client.submit_transaction(rpc, false).await {
        Ok(id) => {
            println!("\n💸 UNLOCKED — the contract released {:.2} TKAS because the oracle attested KAS ≥ $0.02.", payout as f64 / 1e8);
            println!("   NO human decided this — Kaspa's L1 verified the oracle's signature + the price condition.");
            println!("   tx {id}");
            println!("\n✅ A Kaspa contract just consumed a live price oracle, on-chain.");
        }
        Err(e) => { eprintln!("\n✗ spend rejected: {e}"); eprintln!("  (tune ComputeBudget/net_fee, or the script — re-run the spend)"); std::process::exit(1); }
    }
    Ok(())
}

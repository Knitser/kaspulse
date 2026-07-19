//! kaspulse gate — the guide's STEPWISE price-gate CLI. Same covenant that
//! `consumer_live` proved on TN10, pulled apart into inspectable steps so a
//! reader can run one command at a time and see every artifact (keys, redeem
//! hex, P2SH address, txids) along the way. All script comes from kaspulse-sdk
//! (`covenant::*`) — one source of truth, proven on TN10.
//!
//!   cargo run --bin gate --features onchain -- <subcommand>
//!     keygen  [--n 3] [--dir .]                 write gate-node-{i}.key, print pubkeys
//!     address --strike <USD> [--dir .]          print redeem hex + TN10 P2SH address
//!     deploy  --strike <USD> --value <KAS> [--dir .]   fund the P2SH from ~/.kaspulse/tn10.key
//!     spend   --strike <USD> [--dir .]          sign the live oracle price, build witness, spend
//!     demo    --strike <USD> --value <KAS>      deploy → wait for confirm → spend
//!
//! HONEST: this uses a DEMO committee (local gate-node-*.key files) — the
//! hosted committee signs the v2 message string, not blake2b(price_bytes);
//! see /guide.html#honest.

#![allow(deprecated)]
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
use kaspa_txscript::pay_to_address_script;
use kaspa_wrpc_client::{client::ConnectOptions, prelude::{NetworkId, NetworkType}, KaspaRpcClient, Resolver, WrpcEncoding};
use kaspulse_sdk::covenant::{p2sh_address, p2sh_script, price_bytes, price_gate_redeem, price_gate_witness};
use secp256k1::{Keypair, Message, SECP256K1};
use std::time::Duration;

const FEE: u64 = 500_000;
const EXPLORER: &str = "https://explorer-tn10.kaspa.org/txs";

fn blake32(b: &[u8]) -> [u8; 32] { let h = blake2b_simd::Params::new().hash_length(32).hash(b); let mut o = [0u8; 32]; o.copy_from_slice(h.as_bytes()); o }
fn oracle_base() -> String { std::env::var("KASPULSE_BASE").unwrap_or_else(|_| "http://localhost:8080".into()) }

// ---------- args: `gate <sub> [--flag value]...` ----------
struct Args { sub: String, flags: Vec<(String, String)> }
impl Args {
    fn parse() -> Result<Args> {
        let mut it = std::env::args().skip(1);
        let sub = it.next().context(USAGE)?;
        let mut flags = Vec::new();
        while let Some(k) = it.next() {
            let k = k.strip_prefix("--").with_context(|| format!("expected --flag, got {k}\n{USAGE}"))?.to_string();
            let v = it.next().with_context(|| format!("--{k} needs a value"))?;
            flags.push((k, v));
        }
        Ok(Args { sub, flags })
    }
    fn get(&self, k: &str) -> Option<&str> { self.flags.iter().find(|(f, _)| f == k).map(|(_, v)| v.as_str()) }
    fn strike_e8(&self) -> Result<i64> {
        let s: f64 = self.get("strike").context("--strike <USD> is required")?.parse().context("--strike must be a number")?;
        if !(s > 0.0) { bail!("--strike must be > 0"); }
        Ok((s * 1e8).round() as i64)
    }
    fn dir(&self) -> String { self.get("dir").unwrap_or(".").to_string() }
}
const USAGE: &str = "usage: gate <keygen|address|deploy|spend|demo> [--strike USD] [--value KAS] [--n 3] [--dir .]";

// ---------- demo committee keys (gate-node-{i}.key, same hex format as the oracle's node keys) ----------
fn key_path(dir: &str, i: usize) -> String { format!("{}/gate-node-{i}.key", dir.trim_end_matches('/')) }
fn keygen(dir: &str, n: usize) -> Result<()> {
    for i in 0..n {
        let path = key_path(dir, i);
        if std::path::Path::new(&path).exists() { println!("keep  {path} (exists — delete it to regenerate)"); continue; }
        let kp = Keypair::new(SECP256K1, &mut secp256k1::rand::thread_rng());
        std::fs::write(&path, hex::encode(kp.secret_key().secret_bytes())).with_context(|| format!("write {path}"))?;
        println!("wrote {path}");
    }
    for (i, kp) in load_committee(dir)?.iter().enumerate() {
        println!("node {i} x-only pubkey: {}", hex::encode(kp.public_key().x_only_public_key().0.serialize()));
    }
    Ok(())
}
fn load_committee(dir: &str) -> Result<Vec<Keypair>> {
    let mut keys = Vec::new();
    for i in 0.. {
        let path = key_path(dir, i);
        if !std::path::Path::new(&path).exists() { break; }
        let raw = std::fs::read_to_string(&path)?;
        let sk = secp256k1::SecretKey::from_slice(&hex::decode(raw.trim()).with_context(|| format!("{path}: not hex"))?)
            .with_context(|| format!("{path}: not a valid secret key"))?;
        keys.push(Keypair::from_secret_key(SECP256K1, &sk));
    }
    if keys.is_empty() { bail!("no gate-node-*.key in {dir} — run `gate keygen` first"); }
    Ok(keys)
}
fn committee_pks(keys: &[Keypair]) -> Vec<[u8; 32]> {
    keys.iter().map(|k| k.public_key().x_only_public_key().0.serialize()).collect()
}
fn gate_redeem(dir: &str, strike_e8: i64) -> Result<(Vec<Keypair>, Vec<u8>, Address)> {
    let keys = load_committee(dir)?;
    let redeem = price_gate_redeem(&committee_pks(&keys), strike_e8);
    let addr = p2sh_address(&redeem, Prefix::Testnet).map_err(|e| anyhow::anyhow!(e))?;
    Ok((keys, redeem, addr))
}

// ---------- funded TN10 wallet + node connection (same paths as consumer_live) ----------
fn load_key() -> Result<Keypair> {
    let home = std::env::var("HOME").unwrap_or_default();
    let raw = std::fs::read_to_string(format!("{home}/.kaspulse/tn10.key")).or_else(|_| std::fs::read_to_string("/tmp/kascov-lab-key.hex")).context("need the funded TN10 key (~/.kaspulse/tn10.key)")?;
    Ok(Keypair::from_secret_key(SECP256K1, &secp256k1::SecretKey::from_slice(&hex::decode(raw.trim())?)?))
}
fn addr_of(k: &Keypair) -> Address { Address::new(Prefix::Testnet, AddrVersion::PubKey, &k.public_key().x_only_public_key().0.serialize()) }
async fn connect() -> Result<KaspaRpcClient> {
    let net = NetworkId::with_suffix(NetworkType::Testnet, 10);
    let c = KaspaRpcClient::new(WrpcEncoding::Borsh, None, Some(Resolver::default()), Some(net), None)?;
    c.connect(Some(ConnectOptions { block_async_connect: true, connect_timeout: Some(Duration::from_millis(15_000)), ..Default::default() })).await?;
    Ok(c)
}

// ---------- deploy: fund the P2SH ----------
async fn deploy(client: &KaspaRpcClient, key: &Keypair, redeem: &[u8], value: u64) -> Result<kaspa_consensus_core::tx::TransactionId> {
    let me = addr_of(key);
    let my_spk = pay_to_address_script(&me);
    let p2sh = p2sh_script(redeem);
    let utxos = client.get_utxos_by_addresses(vec![me.clone().into()]).await?;
    let fund = utxos.iter().filter(|u| u.utxo_entry.covenant_id.is_none()).max_by_key(|u| u.utxo_entry.amount).context("no funding UTXO — fund the TN10 address first")?;
    if fund.utxo_entry.amount < value + FEE { bail!("largest UTXO ({:.2} TKAS) < value + fee", fund.utxo_entry.amount as f64 / 1e8); }
    let change = fund.utxo_entry.amount - value - FEE;
    let mut outs = vec![TransactionOutput::new(value, p2sh)];
    if change >= 100_000 { outs.push(TransactionOutput::new(change, my_spk)); }
    let inp = TransactionInput::new(TransactionOutpoint::new(fund.outpoint.transaction_id, fund.outpoint.index), vec![], 0, 1);
    let tx = Transaction::new(TX_VERSION_TOCCATA, vec![inp], outs, 0, SUBNETWORK_ID_NATIVE, 0, vec![]);
    let entry = UtxoEntry::new(fund.utxo_entry.amount, fund.utxo_entry.script_public_key.clone(), fund.utxo_entry.block_daa_score, fund.utxo_entry.is_coinbase, None);
    let signed = sign(MutableTransaction::with_entries(tx, vec![entry]), *key);
    let rpc: kaspa_rpc_core::RpcTransaction = (&signed.tx).into();
    let id = client.submit_transaction(rpc, false).await.context("deploy failed")?;
    println!("🎟️  DEPLOYED — {:.2} TKAS locked behind the price gate", value as f64 / 1e8);
    println!("   tx {id}");
    println!("   {EXPLORER}/{id}");
    Ok(signed.tx.id())
}

// ---------- spend: sign the LIVE oracle price with the demo committee, unlock ----------
async fn spend(client: &KaspaRpcClient, key: &Keypair, keys: &[Keypair], redeem: &[u8], p2sh_addr: &Address, strike_e8: i64) -> Result<()> {
    // fetch the live signed price from the oracle and VERIFY it (SDK: sigs + field binding + freshness)
    let base = oracle_base();
    let feed = kaspulse_sdk::fetch(&base, "KAS/USD").map_err(|e| anyhow::anyhow!("fetch {base}/v1/feed/KAS-USD: {e}"))?;
    let px = feed.checked_value_fresh(Duration::from_secs(60)).map_err(|e| anyhow::anyhow!("refusing to use unverified price: {e}"))?;
    let price_e8 = (px * 1e8).round() as i64;
    println!("oracle KAS/USD = ${px:.6} (verified: {}-of-{} sigs + field binding)  ·  strike ${:.6}", feed.threshold, feed.signers.len(), strike_e8 as f64 / 1e8);
    if price_e8 < strike_e8 { bail!("price < strike — the gate would (correctly) refuse this spend; pick a lower --strike"); }

    // the DEMO committee signs blake2b(price_bytes) — the form the script verifies
    let pb = price_bytes(price_e8);
    let msg_hash = blake32(&pb);
    let sigs: Vec<Vec<u8>> = keys.iter().map(|k| k.sign_schnorr(Message::from_digest_slice(&msg_hash).unwrap()).as_ref().to_vec()).collect();
    let sig_script = price_gate_witness(&sigs, price_e8, redeem);
    println!("witness: [{} sigs, price_bytes({price_e8}), redeem] = {} bytes", sigs.len(), sig_script.len());

    let me = addr_of(key);
    let my_spk = pay_to_address_script(&me);
    let states = client.get_utxos_by_addresses(vec![p2sh_addr.clone().into()]).await?;
    let state = states.first().context("coin not in UTXO set yet — wait ~25s for confirm, then re-run `gate spend`")?;
    let mine = client.get_utxos_by_addresses(vec![me.clone().into()]).await?;
    let feeu = mine.iter().filter(|u| u.utxo_entry.covenant_id.is_none() && u.utxo_entry.amount > 10_000_000).max_by_key(|u| u.utxo_entry.amount).context("no fee UTXO")?;

    let net_fee: u64 = 5_000_000;
    let payout = state.utxo_entry.amount + feeu.utxo_entry.amount - net_fee;
    let gate_in = TransactionInput::new_with_mass(TransactionOutpoint::new(state.outpoint.transaction_id, state.outpoint.index), vec![], 0, ComputeCommit::ComputeBudget(ComputeBudget(120)));
    let fee_in = TransactionInput::new_with_mass(TransactionOutpoint::new(feeu.outpoint.transaction_id, feeu.outpoint.index), vec![], 0, ComputeCommit::ComputeBudget(ComputeBudget(50)));
    let tx = Transaction::new(TX_VERSION_TOCCATA, vec![gate_in, fee_in], vec![TransactionOutput::new(payout, my_spk)], 0, SUBNETWORK_ID_NATIVE, 0, vec![]);
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
    mtx.tx.inputs[1].signature_script = kaspa_txscript::script_builder::ScriptBuilder::new().add_data(&s1f)?.drain();

    let rpc: kaspa_rpc_core::RpcTransaction = (&mtx.tx).into();
    match client.submit_transaction(rpc, false).await {
        Ok(id) => {
            println!("\n💸 UNLOCKED — released {:.2} TKAS: Kaspa L1 verified {} committee sigs + price ≥ strike.", payout as f64 / 1e8, keys.len());
            println!("   tx {id}");
            println!("   {EXPLORER}/{id}");
            Ok(())
        }
        Err(e) => { eprintln!("\n✗ spend rejected: {e}"); eprintln!("  (tune ComputeBudget(120)/net_fee — the coin is deployed; re-run `gate spend` to retry)"); std::process::exit(1); }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("kaspulse gate — demo committee (local keys) — the hosted committee signs the v2");
    println!("message string, not price_bytes; see /guide.html#honest\n");
    let a = Args::parse()?;
    match a.sub.as_str() {
        "keygen" => {
            let n: usize = a.get("n").unwrap_or("3").parse().context("--n must be an integer")?;
            keygen(&a.dir(), n)
        }
        "address" => {
            let (_, redeem, addr) = gate_redeem(&a.dir(), a.strike_e8()?)?;
            println!("redeem ({} bytes): {}", redeem.len(), hex::encode(&redeem));
            println!("TN10 P2SH address: {addr}");
            println!("\nnext: gate deploy --strike {} --value 3", a.get("strike").unwrap());
            Ok(())
        }
        "deploy" => {
            let (_, redeem, addr) = gate_redeem(&a.dir(), a.strike_e8()?)?;
            let value = (a.get("value").context("--value <KAS> is required")?.parse::<f64>().context("--value must be a number")? * 1e8) as u64;
            let key = load_key()?;
            let client = connect().await?;
            println!("connected to TN10 · covenant P2SH {addr}\n");
            deploy(&client, &key, &redeem, value).await?;
            println!("\nnext (after ~25s confirm): gate spend --strike {}", a.get("strike").unwrap());
            Ok(())
        }
        "spend" => {
            let strike_e8 = a.strike_e8()?;
            let (keys, redeem, addr) = gate_redeem(&a.dir(), strike_e8)?;
            let key = load_key()?;
            let client = connect().await?;
            println!("connected to TN10 · covenant P2SH {addr}\n");
            spend(&client, &key, &keys, &redeem, &addr, strike_e8).await
        }
        "demo" => {
            let strike_e8 = a.strike_e8()?;
            let dir = a.dir();
            if load_committee(&dir).is_err() { keygen(&dir, 3)?; println!(); }
            let (keys, redeem, addr) = gate_redeem(&dir, strike_e8)?;
            let value = (a.get("value").context("--value <KAS> is required")?.parse::<f64>().context("--value must be a number")? * 1e8) as u64;
            let key = load_key()?;
            let client = connect().await?;
            println!("connected to TN10 · covenant P2SH {addr}\n");
            let deploy_id = deploy(&client, &key, &redeem, value).await?;
            print!("\n   waiting for the coin to confirm");
            let mut confirmed = false;
            for _ in 0..12 {
                tokio::time::sleep(Duration::from_secs(10)).await;
                print!("."); use std::io::Write as _; std::io::stdout().flush().ok();
                let states = client.get_utxos_by_addresses(vec![addr.clone().into()]).await?;
                if states.iter().any(|u| u.outpoint.transaction_id == deploy_id) { confirmed = true; break; }
            }
            println!();
            if !confirmed { bail!("coin not in UTXO set after 120s — re-run `gate spend --strike {}` once it confirms", a.get("strike").unwrap()); }
            spend(&client, &key, &keys, &redeem, &addr, strike_e8).await
        }
        other => bail!("unknown subcommand `{other}`\n{USAGE}"),
    }
}

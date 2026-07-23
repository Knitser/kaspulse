//! kaspulse standing — standing on-chain price feed for Kaspa TN10.
//!
//! Polls the hosted oracle, verifies off-chain + covenant-domain signatures,
//! and publishes a standing price update on **deviation (≥0.5%) or heartbeat
//! (60s)**. Batches majors into one merkle-root in the payload.
//!
//! Usage:
//!   cargo run --bin standing --features onchain -- [oracle_base]
//!
//! Env:
//!   KASPULSE_STANDING_PAIR   default KAS/USD
//!   KASPULSE_DEV_PCT         deviation trigger (default 0.005 = 0.5%)
//!   KASPULSE_HEARTBEAT_S     heartbeat seconds (default 60)
//!   KASPULSE_DRY_RUN=1       verify + print, no broadcast

use anyhow::{bail, Context, Result};
use kaspa_addresses::{Address, Prefix, Version as AddrVersion};
use kaspa_consensus_core::{
    constants::TX_VERSION_TOCCATA,
    sign::sign,
    subnets::SUBNETWORK_ID_NATIVE,
    tx::{MutableTransaction, Transaction, TransactionInput, TransactionOutpoint, TransactionOutput, UtxoEntry},
};
use kaspa_rpc_core::api::rpc::RpcApi;
use kaspa_txscript::pay_to_address_script;
use kaspa_wrpc_client::{client::ConnectOptions, prelude::{NetworkId, NetworkType}, KaspaRpcClient, Resolver, WrpcEncoding};
use kaspulse_sdk::{fetch, fetch_committee, Feed};
use secp256k1::{Keypair, SECP256K1};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const FEE: u64 = 500_000;
const DEFAULT_PAIR: &str = "KAS/USD";
const DEFAULT_DEV: f64 = 0.005;
const DEFAULT_HB: u64 = 60;

fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() }

fn load_funding_key() -> Result<Keypair> {
    let home = std::env::var("HOME").unwrap_or_default();
    let raw = std::fs::read_to_string(format!("{home}/.kaspulse/tn10.key"))
        .or_else(|_| std::fs::read_to_string("/tmp/kascov-lab-key.hex"))
        .context("need a funded TN10 key at ~/.kaspulse/tn10.key")?;
    let sk = secp256k1::SecretKey::from_slice(&hex::decode(raw.trim())?)?;
    Ok(Keypair::from_secret_key(SECP256K1, &sk))
}

fn addr_of(kp: &Keypair) -> Address {
    Address::new(Prefix::Testnet, AddrVersion::PubKey, &kp.x_only_public_key().0.serialize())
}

/// Merkle root over sorted `pair|mant|expo` leaves (blake2b-256 pairwise).
fn merkle_root(feeds: &[Feed]) -> [u8; 32] {
    let mut leaves: Vec<[u8; 32]> = feeds.iter().map(|f| {
        let s = format!("{}|{}|{}", f.pair, f.mant, f.expo);
        let h = blake2b_simd::Params::new().hash_length(32).hash(s.as_bytes());
        let mut out = [0u8; 32]; out.copy_from_slice(h.as_bytes()); out
    }).collect();
    leaves.sort();
    if leaves.is_empty() {
        return [0u8; 32];
    }
    while leaves.len() > 1 {
        if leaves.len() % 2 == 1 { leaves.push(*leaves.last().unwrap()); }
        let mut next = Vec::with_capacity(leaves.len() / 2);
        for chunk in leaves.chunks(2) {
            let mut buf = [0u8; 64];
            buf[..32].copy_from_slice(&chunk[0]);
            buf[32..].copy_from_slice(&chunk[1]);
            let h = blake2b_simd::Params::new().hash_length(32).hash(&buf);
            let mut out = [0u8; 32]; out.copy_from_slice(h.as_bytes());
            next.push(out);
        }
        leaves = next;
    }
    leaves[0]
}

fn payload_bytes(pair: &str, feed: &Feed, root: &[u8; 32]) -> Vec<u8> {
    // standing/v1|PAIR|mant|expo|price_e8|signed_ts|signed_round|merkle_root_hex
    format!(
        "standing/v1|{pair}|{}|{}|{}|{}|{}|{}",
        feed.mant, feed.expo, feed.price_e8, feed.signed_ts, feed.signed_round, hex::encode(root)
    ).into_bytes()
}

async fn connect() -> Result<KaspaRpcClient> {
    let net = NetworkId::with_suffix(NetworkType::Testnet, 10);
    let c = KaspaRpcClient::new(WrpcEncoding::Borsh, None, Some(Resolver::default()), Some(net), None)?;
    c.connect(Some(ConnectOptions { block_async_connect: true, connect_timeout: Some(Duration::from_millis(15_000)), ..Default::default() })).await?;
    Ok(c)
}

async fn publish_payload(client: &KaspaRpcClient, kp: &Keypair, payload: Vec<u8>) -> Result<String> {
    let my = addr_of(kp);
    let my_spk = pay_to_address_script(&my);
    let utxos = client.get_utxos_by_addresses(vec![my.clone()]).await?;
    let fund = utxos.iter().filter(|u| u.utxo_entry.covenant_id.is_none() && u.utxo_entry.amount > FEE + 100_000)
        .max_by_key(|u| u.utxo_entry.amount).context("no funding UTXO")?;
    let change = fund.utxo_entry.amount - FEE;
    let inp = TransactionInput::new(TransactionOutpoint::new(fund.outpoint.transaction_id, fund.outpoint.index), vec![], 0, 1);
    let out = TransactionOutput::new(change, my_spk);
    // Kaspa carries arbitrary bytes in the tx payload field (same pattern as onchain/latency)
    let tx = Transaction::new(TX_VERSION_TOCCATA, vec![inp], vec![out], 0, SUBNETWORK_ID_NATIVE, 0, payload);
    let entry = UtxoEntry::new(fund.utxo_entry.amount, fund.utxo_entry.script_public_key.clone(), fund.utxo_entry.block_daa_score, fund.utxo_entry.is_coinbase, None);
    let signed = sign(MutableTransaction::with_entries(tx, vec![entry]), *kp);
    let rpc: kaspa_rpc_core::RpcTransaction = (&signed.tx).into();
    let id = client.submit_transaction(rpc, false).await.context("standing submit failed")?;
    Ok(id.to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    let base = std::env::args().nth(1).unwrap_or_else(|| "https://pulse.kascov.io".into());
    let pair = std::env::var("KASPULSE_STANDING_PAIR").unwrap_or_else(|_| DEFAULT_PAIR.into());
    let dev_pct: f64 = std::env::var("KASPULSE_DEV_PCT").ok().and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_DEV);
    let hb_s: u64 = std::env::var("KASPULSE_HEARTBEAT_S").ok().and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_HB);
    let dry = std::env::var("KASPULSE_DRY_RUN").map_or(false, |v| v == "1");

    println!("kaspulse standing — pair={pair} · deviation={}% · heartbeat={hb_s}s · oracle={base}", dev_pct * 100.0);
    if dry { println!("DRY RUN — will not broadcast"); }

    let committee = fetch_committee(&base).context("fetch /v1/committee")?;
    println!("committee: {} signers, threshold {}", committee.signers.len(), committee.threshold);

    let kp = if dry { None } else { Some(load_funding_key()?) };
    let client = if dry { None } else { Some(connect().await?) };

    let mut last_price: Option<f64> = None;
    let mut last_pub_ts: u64 = 0;

    loop {
        let feed = match fetch(&base, &pair) {
            Ok(f) => f,
            Err(e) => { eprintln!("fetch error: {e}"); std::thread::sleep(Duration::from_secs(5)); continue; }
        };
        if let Err(e) = feed.verify_with_committee(&committee) {
            eprintln!("verify failed: {e}");
            std::thread::sleep(Duration::from_secs(5));
            continue;
        }
        match feed.verify_covenant() {
            Ok(pe8) => println!("covenant ok · price_e8={pe8}"),
            Err(e) => eprintln!("covenant warn: {e} (continuing with v2 sigs)"),
        }

        let price = feed.value();
        let ts = now();
        let drifted = last_price.map(|lp| lp > 0.0 && (price - lp).abs() / lp >= dev_pct).unwrap_or(true);
        let heartbeat = ts.saturating_sub(last_pub_ts) >= hb_s;

        if !(drifted || heartbeat) {
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }

        let majors: Vec<Feed> = ["KAS/USD", "BTC/USD", "ETH/USD"].iter().filter_map(|p| fetch(&base, p).ok()).collect();
        let root = merkle_root(&majors);
        let payload = payload_bytes(&pair, &feed, &root);
        println!(
            "publish · {} · price={:.8} · reason={} · merkle={}",
            pair, price,
            if drifted { "deviation" } else { "heartbeat" },
            hex::encode(root)
        );

        if dry {
            println!("  payload ({} bytes): {}", payload.len(), String::from_utf8_lossy(&payload));
        } else {
            let client = client.as_ref().unwrap();
            let kp = kp.as_ref().unwrap();
            match publish_payload(client, kp, payload).await {
                Ok(id) => println!("  submitted {id}"),
                Err(e) => { eprintln!("  submit failed: {e}"); bail!("{e}"); }
            }
        }

        last_price = Some(price);
        last_pub_ts = ts;
        std::thread::sleep(Duration::from_secs(2));
    }
}

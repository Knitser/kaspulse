//! kaspulse latency — measure REAL Kaspa inclusion latency, not the brochure.
//! Submits a self-payment carrying a kaspulse payload to TN10, then polls the
//! node until the new UTXO is visible in the UTXO set (i.e. the tx is in an
//! accepted block). Reports submit→visible wall time, three rounds.
//!
//! Run: cargo run --release --bin latency --features onchain

use anyhow::{Context, Result};
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
use secp256k1::{Keypair, SECP256K1};
use std::time::{Duration, Instant};

const FEE: u64 = 500_000;

fn load_key() -> Result<Keypair> {
    let home = std::env::var("HOME").unwrap_or_default();
    let raw = std::fs::read_to_string(format!("{home}/.kaspulse/tn10.key"))
        .or_else(|_| std::fs::read_to_string("/tmp/kascov-lab-key.hex"))
        .context("need a funded TN10 key at ~/.kaspulse/tn10.key")?;
    let sk = secp256k1::SecretKey::from_slice(&hex::decode(raw.trim())?)?;
    Ok(Keypair::from_secret_key(SECP256K1, &sk))
}

#[tokio::main]
async fn main() -> Result<()> {
    let key = load_key()?;
    let me = Address::new(Prefix::Testnet, AddrVersion::PubKey, &key.public_key().x_only_public_key().0.serialize());
    let spk = pay_to_address_script(&me);
    let net = NetworkId::with_suffix(NetworkType::Testnet, 10);
    let client = KaspaRpcClient::new(WrpcEncoding::Borsh, None, Some(Resolver::default()), Some(net), None)?;
    client.connect(Some(ConnectOptions { block_async_connect: true, connect_timeout: Some(Duration::from_millis(15_000)), ..Default::default() })).await?;
    println!("connected to TN10 (public node via resolver — own node would be faster)\n");

    // start from our largest plain UTXO, then chain each round onto the last change output
    let utxos = client.get_utxos_by_addresses(vec![me.clone().into()]).await?;
    let f = utxos.iter().filter(|u| u.utxo_entry.covenant_id.is_none()).max_by_key(|u| u.utxo_entry.amount).context("no UTXO")?;
    let mut outpoint = TransactionOutpoint::new(f.outpoint.transaction_id, f.outpoint.index);
    let mut amount = f.utxo_entry.amount;
    let mut entry_spk = f.utxo_entry.script_public_key.clone();
    let (mut daa, mut coinbase) = (f.utxo_entry.block_daa_score, f.utxo_entry.is_coinbase);

    let mut results = Vec::new();
    for round in 1..=3u32 {
        let value = amount - FEE;
        let out = TransactionOutput::new(value, spk.clone());
        let input = TransactionInput::new(outpoint, vec![], 0, 1);
        let payload = format!("kaspulse-latency-probe-{round}").into_bytes();
        let tx = Transaction::new(TX_VERSION_TOCCATA, vec![input], vec![out], 0, SUBNETWORK_ID_NATIVE, 0, payload);
        let entry = UtxoEntry::new(amount, entry_spk.clone(), daa, coinbase, None);
        let signed = sign(MutableTransaction::with_entries(tx, vec![entry]), key);
        let txid = signed.tx.id();

        let t0 = Instant::now();
        let rpc_tx: kaspa_rpc_core::RpcTransaction = (&signed.tx).into();
        client.submit_transaction(rpc_tx, false).await.context("submit failed")?;
        let t_submit = t0.elapsed().as_millis();

        // poll until the new UTXO is visible in the UTXO set (tx accepted in a block)
        let visible_ms = loop {
            let us = client.get_utxos_by_addresses(vec![me.clone().into()]).await?;
            if us.iter().any(|u| u.outpoint.transaction_id == txid && u.outpoint.index == 0) {
                break t0.elapsed().as_millis();
            }
            if t0.elapsed() > Duration::from_secs(30) { anyhow::bail!("round {round}: not visible after 30s"); }
            tokio::time::sleep(Duration::from_millis(40)).await;
        };
        println!("round {round}: submit(rpc round-trip) {t_submit:>4}ms · tx VISIBLE IN UTXO SET at {visible_ms:>5}ms");
        results.push(visible_ms);

        outpoint = TransactionOutpoint::new(txid, 0);
        amount = value;
        entry_spk = spk.clone();
        daa = 0; coinbase = false;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let avg: u128 = results.iter().sum::<u128>() / results.len() as u128;
    println!("\nmeasured on TN10 via a PUBLIC node: avg {avg}ms from submit to accepted-in-UTXO-set");
    println!("(includes RPC round-trips both ways; a co-located own node removes most of that)");
    Ok(())
}

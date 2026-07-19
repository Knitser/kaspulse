//! kaspulse-sdk — consume the kaspulse oracle.
//!
//! Two things a builder needs:
//!   1. OFF-CHAIN: fetch the latest signed price and VERIFY it yourself
//!      (`fetch` + `Feed::verify`) — never trust the API, check the signatures.
//!   2. ON-CHAIN: build a price-gated covenant so Kaspa L1 enforces the oracle
//!      condition at spend time (`covenant::price_gate_redeem`, feature = "covenant").
//!
//! The signed price is `mant × 10^expo` (9 significant digits at any magnitude).
//! Each node signs `schnorr(blake2b("kaspulse/v2|PAIR|mant|expo|ts|round"))`.
//!
//! 0.2.0 hardening: `verify()` now BINDS the signed message's fields to the
//! JSON fields — a server that put one price in `message` and another in
//! `mant`/`expo` used to slip past `checked_value()`. It can't anymore.

use serde::Deserialize;

// ---------------- errors ----------------

/// Typed fetch errors (0.2.0 — `fetch*` used to return `String`).
#[derive(Debug)]
pub enum Error {
    /// The oracle answered 404: no feed with that pair.
    NoSuchFeed(String),
    /// Transport or non-404 HTTP failure.
    Http(String),
    /// The body didn't parse as the expected JSON shape.
    Parse(String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NoSuchFeed(p) => write!(f, "no such feed: {p}"),
            Error::Http(e) => write!(f, "http: {e}"),
            Error::Parse(e) => write!(f, "parse: {e}"),
        }
    }
}
impl std::error::Error for Error {}

// ---------------- feed shapes ----------------

#[derive(Debug, Clone, Deserialize)]
pub struct Source { pub name: String, pub price: f64, #[serde(default)] pub age_ms: u64 }

#[derive(Debug, Clone, Deserialize)]
pub struct Feed {
    pub pair: String,
    pub kind: String,
    pub price: f64,
    pub mant: u64,
    pub expo: i32,
    pub median: f64,
    #[serde(default)] pub sources: Vec<Source>,
    #[serde(default)] pub num_sources: usize,
    #[serde(default)] pub freshest_ms: u64,
    #[serde(default)] pub thin: bool,
    #[serde(default)] pub halted: bool,
    #[serde(default)] pub degraded: bool,
    #[serde(default)] pub peg_ok: Option<bool>,
    pub threshold: usize,
    pub signers: Vec<String>,
    pub signatures: Vec<String>,
    pub message: String,
    #[serde(default)] pub signed_ts: u64,
    #[serde(default)] pub signed_round: u64,
}

#[derive(Debug, Deserialize)]
struct Envelope { feeds: Vec<Feed> }

/// One row of the `/v1/feeds` light catalog (what dashboards poll — a few KB
/// instead of the multi-hundred-KB full envelope).
#[derive(Debug, Clone, Deserialize)]
pub struct CatalogFeed {
    pub pair: String,
    pub kind: String,
    pub price: f64,
    pub num_sources: usize,
    pub halted: bool,
    pub degraded: bool,
    pub thin: bool,
    pub liq_wkas: f64,
    pub spread_bps: f64,
    pub freshest_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    pub round: u64,
    pub timestamp: u64,
    pub count: usize,
    pub feeds: Vec<CatalogFeed>,
}

/// The parsed fields of the pipe-delimited signed message
/// `"kaspulse/v2|PAIR|mant|expo|ts|round"`. These are what the committee
/// actually signed — [`Feed::verify`] requires them to EQUAL the JSON fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedMessage {
    pub pair: String,
    pub mant: u64,
    pub expo: i32,
    pub ts: u64,
    pub round: u64,
}

// strict decimal parses: ASCII digits only — no whitespace, no '+', no empties
fn parse_u64_strict(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) { return None; }
    // canonical form only ("07" ≠ "7") — keeps this verifier exactly as strict
    // as the JS/Python clients, which compare the message fields as strings
    if s.len() > 1 && s.starts_with('0') { return None; }
    s.parse().ok()
}
fn parse_i32_strict(s: &str) -> Option<i32> {
    let t = s.strip_prefix('-').unwrap_or(s);
    if t.is_empty() || !t.bytes().all(|b| b.is_ascii_digit()) { return None; }
    if t.len() > 1 && t.starts_with('0') { return None; }
    if t == "0" && s.starts_with('-') { return None; }
    s.parse().ok()
}

impl Feed {
    /// The exact value a covenant / consumer should treat as the price.
    pub fn value(&self) -> f64 { self.mant as f64 * 10f64.powi(self.expo) }

    /// Strict-parse `self.message` into its signed fields. Rejects anything
    /// that isn't exactly `kaspulse/v2|PAIR|mant|expo|ts|round` with clean
    /// decimal integers (no whitespace, no trailing pipe, no extra segments).
    pub fn signed_message(&self) -> Result<SignedMessage, &'static str> {
        let parts: Vec<&str> = self.message.split('|').collect();
        if parts.len() != 6 { return Err("message: expected 6 pipe-separated segments"); }
        if parts[0] != "kaspulse/v2" { return Err("message: unknown version prefix"); }
        if parts[1].is_empty() { return Err("message: empty pair"); }
        let mant = parse_u64_strict(parts[2]).ok_or("message: bad mant")?;
        let expo = parse_i32_strict(parts[3]).ok_or("message: bad expo")?;
        let ts = parse_u64_strict(parts[4]).ok_or("message: bad ts")?;
        let round = parse_u64_strict(parts[5]).ok_or("message: bad round")?;
        Ok(SignedMessage { pair: parts[1].to_string(), mant, expo, ts, round })
    }

    /// Verify the threshold of node signatures over blake2b(message), AND that
    /// the message's fields equal the JSON fields (field binding — otherwise a
    /// lying server could serve a `mant`/`expo` the signatures don't cover).
    /// Trust NOTHING else.
    pub fn verify(&self) -> Result<(), &'static str> {
        if self.message.is_empty() || self.signers.is_empty() { return Err("empty feed"); }
        // guard flags a careful consumer should honor
        if self.halted { return Err("feed halted (circuit breaker)"); }
        if self.peg_ok == Some(false) { return Err("chain depegged"); }
        // bind the signed message's fields to the JSON fields
        let m = self.signed_message()?;
        if m.pair != self.pair || m.mant != self.mant || m.expo != self.expo || m.ts != self.signed_ts {
            return Err("message/field mismatch — API served fields the signatures don't cover");
        }
        let h = blake2b_simd::Params::new().hash_length(32).hash(self.message.as_bytes());
        let msg = secp256k1::Message::from_digest_slice(h.as_bytes()).map_err(|_| "hash")?;
        let secp = secp256k1::Secp256k1::verification_only();
        let mut valid = 0usize;
        for (pk_hex, sig_hex) in self.signers.iter().zip(self.signatures.iter()) {
            let ok = (|| {
                let pk = secp256k1::XOnlyPublicKey::from_slice(&hex::decode(pk_hex).ok()?).ok()?;
                let sig = secp256k1::schnorr::Signature::from_slice(&hex::decode(sig_hex).ok()?).ok()?;
                Some(secp.verify_schnorr(&sig, &msg, &pk).is_ok())
            })().unwrap_or(false);
            if ok { valid += 1; }
        }
        if valid >= self.threshold { Ok(()) } else { Err("threshold not met") }
    }

    /// Convenience: verified value, or an error explaining why not to use it.
    pub fn checked_value(&self) -> Result<f64, &'static str> { self.verify().map(|_| self.value()) }

    /// Verified value, additionally requiring the SIGNED timestamp to be within
    /// `max_age` of the system clock. Use this in anything that moves money.
    pub fn checked_value_fresh(&self, max_age: std::time::Duration) -> Result<f64, &'static str> {
        self.verify()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).map_err(|_| "system clock before 1970")?.as_secs();
        if now.saturating_sub(self.signed_ts) > max_age.as_secs() {
            return Err("signed price is stale (signed_ts older than max_age)");
        }
        Ok(self.value())
    }
}

// ---------------- fetch (HTTP, /v1) ----------------

fn get_json<T: serde::de::DeserializeOwned>(url: &str, pair_for_404: Option<&str>) -> Result<T, Error> {
    match ureq::get(url).call() {
        Ok(r) => r.into_json().map_err(|e| Error::Parse(e.to_string())),
        Err(ureq::Error::Status(404, _)) if pair_for_404.is_some() =>
            Err(Error::NoSuchFeed(pair_for_404.unwrap().to_string())),
        Err(e) => Err(Error::Http(e.to_string())),
    }
}

/// Fetch one feed, e.g. `fetch("http://localhost:8080", "KAS/USD")`.
/// Unknown pair → `Error::NoSuchFeed` (the oracle answers a real 404).
pub fn fetch(base: &str, pair: &str) -> Result<Feed, Error> {
    let url = format!("{}/v1/feed/{}", base.trim_end_matches('/'), pair.replace('/', "-"));
    get_json(&url, Some(pair))
}

/// Fetch every feed at once (the full envelope — heavy; prefer `fetch_catalog`
/// if you only need prices and health flags).
pub fn fetch_all(base: &str) -> Result<Vec<Feed>, Error> {
    let url = format!("{}/v1/feed", base.trim_end_matches('/'));
    let e: Envelope = get_json(&url, None)?;
    Ok(e.feeds)
}

/// Fetch the light `/v1/feeds` catalog — one small row per pair, built for
/// dashboards and watchers that poll. NOTE: catalog rows are NOT signed;
/// verify a pair's full feed (`fetch` + `checked_value*`) before acting on it.
pub fn fetch_catalog(base: &str) -> Result<Catalog, Error> {
    let url = format!("{}/v1/feeds", base.trim_end_matches('/'));
    get_json(&url, None)
}

// ---------------- on-chain: covenant builders ----------------
/// Price-gated covenant builders, extracted verbatim from the bins proven live
/// on Kaspa TN10 (`consumer_live`, `slash`, `slash_live`). The SDK ships
/// PROVEN script only — no reclaim-timelock bond branch is exposed here; the
/// repo's `slash.rs` marks it unproven, so it stays out until it's proven.
#[cfg(feature = "covenant")]
pub mod covenant {
    use kaspa_txscript::{opcodes::codes::*, script_builder::ScriptBuilder};
    pub use kaspa_addresses::{Address, Prefix};
    pub use kaspa_consensus_core::tx::ScriptPublicKey;

    /// Which side of the strike releases the payout.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Gate {
        /// Release iff price ≥ strike (payouts, options finishing in the money).
        AtOrAbove,
        /// Release iff price ≤ strike (liquidations, stop conditions).
        AtOrBelow,
    }

    // the shared n-of-n committee tail: blake2b(price_bytes) on the alt stack,
    // then OpCheckSigFromStack per key — abort-on-fail for all but the first
    // signer, whose check leaves the final bool.
    fn committee_tail(b: &mut ScriptBuilder, committee: &[[u8; 32]]) {
        b.add_op(OpBlake2b).unwrap().add_op(OpToAltStack).unwrap();
        for pk in committee[1..].iter().rev() {
            b.add_op(OpFromAltStack).unwrap().add_op(OpDup).unwrap().add_op(OpToAltStack).unwrap()
                .add_data(pk).unwrap().add_op(OpCheckSigFromStack).unwrap().add_op(OpVerify).unwrap();
        }
        b.add_op(OpFromAltStack).unwrap().add_data(&committee[0]).unwrap().add_op(OpCheckSigFromStack).unwrap();
    }

    /// Redeem for "release only if all `committee` nodes signed the price AND
    /// price is on `gate`'s side of `strike_e8`". `committee` = x-only node
    /// pubkeys (32 bytes each); the spender provides
    /// `[sig_0..sig_{n-1}, price_bytes]` in the signature script
    /// (price_bytes = minimal LE script-number of price_e8).
    /// The oracle nodes sign `schnorr(blake2b(price_bytes))`.
    pub fn price_gate_redeem_dir(committee: &[[u8; 32]], strike_e8: i64, gate: Gate) -> Vec<u8> {
        assert!(!committee.is_empty());
        let cmp = match gate { Gate::AtOrAbove => OpGreaterThanOrEqual, Gate::AtOrBelow => OpLessThanOrEqual };
        let mut b = ScriptBuilder::new();
        b.add_op(OpDup).unwrap().add_i64(strike_e8).unwrap().add_op(cmp).unwrap().add_op(OpVerify).unwrap();
        committee_tail(&mut b, committee);
        b.drain()
    }

    /// The original ≥-strike gate (proven on TN10 by `consumer_live`).
    /// Identical to `price_gate_redeem_dir(committee, strike_e8, Gate::AtOrAbove)`.
    pub fn price_gate_redeem(committee: &[[u8; 32]], strike_e8: i64) -> Vec<u8> {
        price_gate_redeem_dir(committee, strike_e8, Gate::AtOrAbove)
    }

    /// Range settle: release only if `low_e8 ≤ price ≤ high_e8`, signed by the
    /// committee. Same witness shape as the price gate.
    pub fn range_settle_redeem(committee: &[[u8; 32]], low_e8: i64, high_e8: i64) -> Vec<u8> {
        assert!(!committee.is_empty());
        let mut b = ScriptBuilder::new();
        b.add_op(OpDup).unwrap().add_i64(low_e8).unwrap().add_op(OpGreaterThanOrEqual).unwrap().add_op(OpVerify).unwrap()
            .add_op(OpDup).unwrap().add_i64(high_e8).unwrap().add_op(OpLessThanOrEqual).unwrap().add_op(OpVerify).unwrap();
        committee_tail(&mut b, committee);
        b.drain()
    }

    /// The spend-side signature script for a price gate / range settle:
    /// pushes bottom→top `[sig_0 .. sig_{n-1}, price_bytes(price_e8), redeem]`
    /// — exactly the witness `consumer_live` spent with on TN10.
    pub fn price_gate_witness(sigs: &[Vec<u8>], price_e8: i64, redeem: &[u8]) -> Vec<u8> {
        let mut b = ScriptBuilder::new();
        for s in sigs { b.add_data(s).unwrap(); }
        b.add_data(&price_bytes(price_e8)).unwrap().add_data(redeem).unwrap();
        b.drain()
    }

    /// P2SH script-public-key committing to `redeem`.
    pub fn p2sh_script(redeem: &[u8]) -> ScriptPublicKey {
        kaspa_txscript::pay_to_script_hash_script(redeem)
    }

    /// The bech32 address of the P2SH commit (e.g. `Prefix::Testnet` for TN10).
    pub fn p2sh_address(redeem: &[u8], prefix: Prefix) -> Result<Address, String> {
        kaspa_txscript::extract_script_pub_key_address(&p2sh_script(redeem), prefix)
            .map_err(|e| format!("{e:?}"))
    }

    /// Minimal little-endian script-number encoding of `price_e8` (what the
    /// spender pushes and the nodes sign the blake2b of).
    pub fn price_bytes(price_e8: i64) -> Vec<u8> {
        if price_e8 == 0 { return vec![]; }
        let neg = price_e8 < 0; let mut abs = price_e8.unsigned_abs(); let mut out = Vec::new();
        while abs > 0 { out.push((abs & 0xff) as u8); abs >>= 8; }
        if out.last().unwrap() & 0x80 != 0 { out.push(if neg { 0x80 } else { 0 }); } else if neg { *out.last_mut().unwrap() |= 0x80; }
        out
    }

    /// Equivocation-bond helpers, extracted from the `slash` / `slash_live`
    /// bins (slashing path proven on TN10). A node posts a bond behind
    /// `bond_redeem(node_pk)` and signs a 24-byte [`attestation_record`] per
    /// feed round; two same-slot different-price records signed by the same
    /// key let ANYONE seize the bond — verified purely by Kaspa L1 script.
    ///
    /// NO reclaim-timelock branch is exposed: the honest node's bond-reclaim
    /// path is marked unproven in `slash.rs`, and this SDK ships proven script
    /// only.
    pub mod bond {
        use kaspa_txscript::{opcodes::codes::*, script_builder::ScriptBuilder};

        /// 24 bytes: slot(blake2b(pair)[0..8] ‖ round_be[8..16]) ‖ price(mant_be[16..24]).
        pub fn attestation_record(pair: &str, round: u64, mant: u64) -> [u8; 24] {
            let h = blake2b_simd::Params::new().hash_length(32).hash(pair.as_bytes());
            let mut r = [0u8; 24];
            r[..8].copy_from_slice(&h.as_bytes()[..8]);
            r[8..16].copy_from_slice(&round.to_be_bytes());
            r[16..24].copy_from_slice(&mant.to_be_bytes());
            r
        }

        /// The bond redeem — slashes iff two valid, same-slot, different-price
        /// records signed by `node_pk`. Witness leaves `[rec1, sig1, rec2, sig2]`
        /// on the stack (sig2 top) — see [`slash_witness`].
        pub fn bond_redeem(node_pk: &[u8; 32]) -> Vec<u8> {
            let mut b = ScriptBuilder::new();
            // verify sig2 over rec2, stashing a copy of rec2 on the alt-stack
            b.add_op(OpSwap).unwrap().add_op(OpDup).unwrap().add_op(OpToAltStack).unwrap().add_op(OpBlake2b).unwrap()
                .add_data(node_pk).unwrap().add_op(OpCheckSigFromStack).unwrap().add_op(OpVerify).unwrap();
            // verify sig1 over rec1, stashing a copy of rec1
            b.add_op(OpSwap).unwrap().add_op(OpDup).unwrap().add_op(OpToAltStack).unwrap().add_op(OpBlake2b).unwrap()
                .add_data(node_pk).unwrap().add_op(OpCheckSigFromStack).unwrap().add_op(OpVerify).unwrap();
            // restore rec1, rec2 → stack [rec1, rec2]
            b.add_op(OpFromAltStack).unwrap().add_op(OpFromAltStack).unwrap();
            // same slot?  rec1[0..16] == rec2[0..16]
            b.add_op(Op2Dup).unwrap()
                .add_i64(0).unwrap().add_i64(16).unwrap().add_op(OpSubstr).unwrap()          // slot2
                .add_op(OpSwap).unwrap().add_i64(0).unwrap().add_i64(16).unwrap().add_op(OpSubstr).unwrap() // slot1
                .add_op(OpEqualVerify).unwrap();
            // different price?  rec1[16..24] != rec2[16..24]
            b.add_i64(16).unwrap().add_i64(24).unwrap().add_op(OpSubstr).unwrap()            // price2
                .add_op(OpSwap).unwrap().add_i64(16).unwrap().add_i64(24).unwrap().add_op(OpSubstr).unwrap() // price1
                .add_op(OpEqual).unwrap().add_op(OpNot).unwrap();
            b.drain()
        }

        /// The slash spend's signature script:
        /// `[rec1, sig1, rec2, sig2, redeem]` bottom→top.
        pub fn slash_witness(rec1: &[u8], sig1: &[u8], rec2: &[u8], sig2: &[u8], redeem: &[u8]) -> Vec<u8> {
            ScriptBuilder::new()
                .add_data(rec1).unwrap().add_data(sig1).unwrap()
                .add_data(rec2).unwrap().add_data(sig2).unwrap()
                .add_data(redeem).unwrap().drain()
        }

        /// Off-chain pre-check of what the script enforces: same 24-byte layout,
        /// same slot (pair-hash ‖ round), DIFFERENT price mantissa.
        pub fn is_equivocation(rec1: &[u8], rec2: &[u8]) -> bool {
            rec1.len() == 24 && rec2.len() == 24 && rec1[..16] == rec2[..16] && rec1[16..24] != rec2[16..24]
        }
    }
}

// ---------------- tests ----------------
#[cfg(test)]
mod tests {
    use super::*;

    // a Feed with valid 2-of-3 signatures over a well-formed v2 message,
    // built from fixed secret keys (no rng needed).
    fn signed_feed() -> Feed {
        let secp = secp256k1::Secp256k1::new();
        let keys: Vec<secp256k1::Keypair> = (1u8..=3)
            .map(|i| secp256k1::Keypair::from_secret_key(&secp, &secp256k1::SecretKey::from_slice(&[i; 32]).unwrap()))
            .collect();
        let (pair, mant, expo, ts, round) = ("KAS/USD", 290_000_000u64, -10i32, 1_752_800_000u64, 7u64);
        let message = format!("kaspulse/v2|{pair}|{mant}|{expo}|{ts}|{round}");
        let h = blake2b_simd::Params::new().hash_length(32).hash(message.as_bytes());
        let msg = secp256k1::Message::from_digest_slice(h.as_bytes()).unwrap();
        let signers: Vec<String> = keys.iter().map(|k| hex::encode(k.public_key().x_only_public_key().0.serialize())).collect();
        let signatures: Vec<String> = keys.iter().map(|k| hex::encode(secp.sign_schnorr_no_aux_rand(&msg, k).as_ref())).collect();
        Feed {
            pair: pair.into(), kind: "major".into(), price: 0.029, mant, expo, median: 0.029,
            sources: vec![], num_sources: 3, freshest_ms: 100, thin: false, halted: false,
            degraded: false, peg_ok: None, threshold: 2, signers, signatures, message,
            signed_ts: ts, signed_round: round,
        }
    }

    #[test]
    fn signed_message_strict_parse() {
        let f = signed_feed();
        let m = f.signed_message().unwrap();
        assert_eq!(m, SignedMessage { pair: "KAS/USD".into(), mant: 290_000_000, expo: -10, ts: 1_752_800_000, round: 7 });

        let with = |msg: &str| { let mut g = f.clone(); g.message = msg.into(); g };
        // reject: trailing pipe (7 segments)
        assert!(with("kaspulse/v2|KAS/USD|290000000|-10|1752800000|7|").signed_message().is_err());
        // reject: 5 segments
        assert!(with("kaspulse/v2|KAS/USD|290000000|-10|1752800000").signed_message().is_err());
        // reject: wrong prefix
        assert!(with("kaspulse/v1|KAS/USD|290000000|-10|1752800000|7").signed_message().is_err());
        // reject: non-integer mant
        assert!(with("kaspulse/v2|KAS/USD|29e7|-10|1752800000|7").signed_message().is_err());
        // reject: whitespace inside a number
        assert!(with("kaspulse/v2|KAS/USD| 290000000|-10|1752800000|7").signed_message().is_err());
        // reject: '+' sign (not emitted by the oracle, not accepted here)
        assert!(with("kaspulse/v2|KAS/USD|+290000000|-10|1752800000|7").signed_message().is_err());
        // reject: empty pair
        assert!(with("kaspulse/v2||290000000|-10|1752800000|7").signed_message().is_err());
        // accept: positive expo
        assert_eq!(with("kaspulse/v2|BTC/USD|118000000|3|1752800000|7").signed_message().unwrap().expo, 3);
    }

    #[test]
    fn verify_accepts_honest_feed_and_binds_fields() {
        let f = signed_feed();
        assert!(f.verify().is_ok());
        assert_eq!(f.checked_value().unwrap(), 290_000_000f64 * 10f64.powi(-10));

        // a lying server: signatures still valid over `message`, but the JSON
        // mant is inflated — 0.1.x accepted this, 0.2.0 must reject it.
        let mut lied = f.clone();
        lied.mant = 580_000_000;
        lied.price = 0.058;
        assert_eq!(lied.verify().unwrap_err(), "message/field mismatch — API served fields the signatures don't cover");
        let mut lied2 = f.clone();
        lied2.pair = "BTC/USD".into();
        assert!(lied2.verify().is_err());
        let mut lied3 = f.clone();
        lied3.signed_ts += 1;
        assert!(lied3.verify().is_err());
        let mut lied4 = f.clone();
        lied4.expo = -9;
        assert!(lied4.verify().is_err());
        // tampered signature bytes → threshold not met
        let mut bad = f.clone();
        bad.signatures = bad.signatures.iter().map(|_| "00".repeat(64)).collect();
        assert_eq!(bad.verify().unwrap_err(), "threshold not met");
    }

    #[test]
    fn checked_value_fresh_rejects_stale() {
        use std::time::{Duration, SystemTime, UNIX_EPOCH};
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        // stale: signed an hour ago per the fixed test vector ts
        let f = signed_feed();
        assert!(f.checked_value_fresh(Duration::from_secs(10)).is_err());
        // fresh: re-sign with ts = now
        let secp = secp256k1::Secp256k1::new();
        let keys: Vec<secp256k1::Keypair> = (1u8..=3)
            .map(|i| secp256k1::Keypair::from_secret_key(&secp, &secp256k1::SecretKey::from_slice(&[i; 32]).unwrap()))
            .collect();
        let mut g = f.clone();
        g.signed_ts = now;
        g.message = format!("kaspulse/v2|KAS/USD|290000000|-10|{now}|7");
        let h = blake2b_simd::Params::new().hash_length(32).hash(g.message.as_bytes());
        let msg = secp256k1::Message::from_digest_slice(h.as_bytes()).unwrap();
        g.signatures = keys.iter().map(|k| hex::encode(secp.sign_schnorr_no_aux_rand(&msg, k).as_ref())).collect();
        assert!(g.checked_value_fresh(Duration::from_secs(30)).is_ok());
    }

    #[cfg(feature = "covenant")]
    mod covenant_tests {
        use crate::covenant::{self, bond, Gate};

        fn committee() -> Vec<[u8; 32]> {
            let secp = secp256k1::Secp256k1::new();
            (1u8..=3).map(|i| {
                secp256k1::Keypair::from_secret_key(&secp, &secp256k1::SecretKey::from_slice(&[i; 32]).unwrap())
                    .public_key().x_only_public_key().0.serialize()
            }).collect()
        }

        #[test]
        fn price_bytes_vectors() {
            assert_eq!(covenant::price_bytes(0), Vec::<u8>::new());
            assert_eq!(covenant::price_bytes(1), vec![0x01]);
            assert_eq!(covenant::price_bytes(127), vec![0x7f]);
            // 0x80 boundary: high bit set needs a sign byte
            assert_eq!(covenant::price_bytes(128), vec![0x80, 0x00]);
            assert_eq!(covenant::price_bytes(255), vec![0xff, 0x00]);
            assert_eq!(covenant::price_bytes(-1), vec![0x81]);
            assert_eq!(covenant::price_bytes(-128), vec![0x80, 0x80]);
            // $0.02 strike
            assert_eq!(covenant::price_bytes(2_000_000), vec![0x80, 0x84, 0x1e]);
        }

        // the 0.1.0 price_gate_redeem, kept verbatim as a reference — the 0.2.0
        // Gate::AtOrAbove path must stay byte-identical (it's proven on TN10).
        fn legacy_price_gate_redeem(committee: &[[u8; 32]], strike_e8: i64) -> Vec<u8> {
            use kaspa_txscript::{opcodes::codes::*, script_builder::ScriptBuilder};
            let mut b = ScriptBuilder::new();
            b.add_op(OpDup).unwrap().add_i64(strike_e8).unwrap().add_op(OpGreaterThanOrEqual).unwrap().add_op(OpVerify).unwrap()
                .add_op(OpBlake2b).unwrap().add_op(OpToAltStack).unwrap();
            for pk in committee[1..].iter().rev() {
                b.add_op(OpFromAltStack).unwrap().add_op(OpDup).unwrap().add_op(OpToAltStack).unwrap()
                    .add_data(pk).unwrap().add_op(OpCheckSigFromStack).unwrap().add_op(OpVerify).unwrap();
            }
            b.add_op(OpFromAltStack).unwrap().add_data(&committee[0]).unwrap().add_op(OpCheckSigFromStack).unwrap();
            b.drain()
        }

        #[test]
        fn gate_at_or_above_byte_equals_legacy() {
            let c = committee();
            assert_eq!(covenant::price_gate_redeem_dir(&c, 2_000_000, Gate::AtOrAbove), legacy_price_gate_redeem(&c, 2_000_000));
            assert_eq!(covenant::price_gate_redeem(&c, 2_000_000), legacy_price_gate_redeem(&c, 2_000_000));
            // AtOrBelow differs only in the comparison opcode
            let above = covenant::price_gate_redeem_dir(&c, 2_000_000, Gate::AtOrAbove);
            let below = covenant::price_gate_redeem_dir(&c, 2_000_000, Gate::AtOrBelow);
            assert_eq!(above.len(), below.len());
            assert_eq!(above.iter().zip(below.iter()).filter(|(a, b)| a != b).count(), 1);
        }

        #[test]
        fn range_settle_shares_the_proven_tail() {
            let c = committee();
            let range = covenant::range_settle_redeem(&c, 1_000_000, 3_000_000);
            let gate = covenant::price_gate_redeem(&c, 1_000_000);
            // both end in the identical committee tail (everything after the strike checks)
            let tail_len = gate.len() - gate.iter().position(|&op| op == 0xaa /* OpBlake2b */).unwrap();
            assert_eq!(range[range.len() - tail_len..], gate[gate.len() - tail_len..]);
        }

        #[test]
        fn witness_matches_consumer_live_shape() {
            use kaspa_txscript::script_builder::ScriptBuilder;
            let sigs: Vec<Vec<u8>> = (0u8..3).map(|i| vec![i; 64]).collect();
            let redeem = covenant::price_gate_redeem(&committee(), 2_000_000);
            let want = ScriptBuilder::new()
                .add_data(&sigs[0]).unwrap().add_data(&sigs[1]).unwrap().add_data(&sigs[2]).unwrap()
                .add_data(&covenant::price_bytes(2_900_000)).unwrap().add_data(&redeem).unwrap().drain();
            assert_eq!(covenant::price_gate_witness(&sigs, 2_900_000, &redeem), want);
        }

        #[test]
        fn attestation_record_layout() {
            let r = bond::attestation_record("KAS/USD", 42, 2_900_000);
            let h = blake2b_simd::Params::new().hash_length(32).hash(b"KAS/USD");
            assert_eq!(&r[..8], &h.as_bytes()[..8]);
            assert_eq!(&r[8..16], &42u64.to_be_bytes());
            assert_eq!(&r[16..24], &2_900_000u64.to_be_bytes());
        }

        #[test]
        fn is_equivocation_truth_table() {
            let r1 = bond::attestation_record("KAS/USD", 42, 2_900_000);
            let r2 = bond::attestation_record("KAS/USD", 42, 5_800_000); // same slot, different price
            let r3 = bond::attestation_record("KAS/USD", 43, 5_800_000); // different round
            let r4 = bond::attestation_record("BTC/USD", 42, 2_900_000); // different pair
            assert!(bond::is_equivocation(&r1, &r2));
            assert!(bond::is_equivocation(&r2, &r1));
            assert!(!bond::is_equivocation(&r1, &r1)); // same record — no conflict
            assert!(!bond::is_equivocation(&r1, &r3)); // honest update across rounds
            assert!(!bond::is_equivocation(&r1, &r4)); // different pair slot
            assert!(!bond::is_equivocation(&r1[..20], &r2[..20])); // wrong length
        }

        #[test]
        fn p2sh_address_is_deterministic() {
            let redeem = covenant::price_gate_redeem(&committee(), 2_000_000);
            let a1 = covenant::p2sh_address(&redeem, covenant::Prefix::Testnet).unwrap();
            let a2 = covenant::p2sh_address(&redeem, covenant::Prefix::Testnet).unwrap();
            assert_eq!(a1, a2);
            assert!(a1.to_string().starts_with("kaspatest:"));
        }
    }
}

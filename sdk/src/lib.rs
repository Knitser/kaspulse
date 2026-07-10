//! kaspulse-sdk — consume the kaspulse oracle.
//!
//! Two things a builder needs:
//!   1. OFF-CHAIN: fetch the latest signed price and VERIFY it yourself
//!      (`fetch` + `Feed::verify`) — never trust the API, check the signatures.
//!   2. ON-CHAIN: build a price-gated covenant so Kaspa L1 enforces the oracle
//!      condition at spend time (`price_gate_redeem`, feature = "covenant").
//!
//! The signed price is `mant × 10^expo` (9 significant digits at any magnitude).
//! Each node signs `schnorr(blake2b("kaspulse/v2|PAIR|mant|expo|ts|round"))`.

use serde::Deserialize;

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
    #[serde(default)] pub peg_ok: Option<bool>,
    pub threshold: usize,
    pub signers: Vec<String>,
    pub signatures: Vec<String>,
    pub message: String,
    #[serde(default)] pub signed_ts: u64,
}

#[derive(Debug, Deserialize)]
struct Envelope { feeds: Vec<Feed> }

/// The exact value a covenant / consumer should treat as the price.
impl Feed {
    pub fn value(&self) -> f64 { self.mant as f64 * 10f64.powi(self.expo) }

    /// Verify the threshold of node signatures over blake2b(message). Trust NOTHING else.
    pub fn verify(&self) -> Result<(), &'static str> {
        if self.message.is_empty() || self.signers.is_empty() { return Err("empty feed"); }
        // guard flags a careful consumer should honor
        if self.halted { return Err("feed halted (circuit breaker)"); }
        if self.peg_ok == Some(false) { return Err("chain depegged"); }
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
}

/// Fetch one feed, e.g. `fetch("http://localhost:8080", "KAS/USD")`.
pub fn fetch(base: &str, pair: &str) -> Result<Feed, String> {
    let url = format!("{}/api/feed/{}", base.trim_end_matches('/'), pair.replace('/', "-"));
    let f: Feed = ureq::get(&url).call().map_err(|e| e.to_string())?.into_json().map_err(|e| e.to_string())?;
    Ok(f)
}
/// Fetch every feed at once.
pub fn fetch_all(base: &str) -> Result<Vec<Feed>, String> {
    let url = format!("{}/api/feed", base.trim_end_matches('/'));
    let e: Envelope = ureq::get(&url).call().map_err(|e| e.to_string())?.into_json().map_err(|e| e.to_string())?;
    Ok(e.feeds)
}

// ---------------- on-chain: price-gated covenant builder ----------------
#[cfg(feature = "covenant")]
pub mod covenant {
    use kaspa_txscript::{opcodes::codes::*, script_builder::ScriptBuilder};

    /// Redeem for a "release only if ≥ `committee.len()` nodes signed the price
    /// AND price ≥ `strike_e8`" covenant. `committee` = x-only node pubkeys (each
    /// 32 bytes); the spender provides `[sig_0..sig_{n-1}, price_bytes]` in the
    /// signature script (price_bytes = minimal LE script-number of price_e8).
    /// The oracle nodes sign `schnorr(blake2b(price_bytes))`.
    pub fn price_gate_redeem(committee: &[[u8; 32]], strike_e8: i64) -> Vec<u8> {
        assert!(!committee.is_empty());
        let mut b = ScriptBuilder::new();
        b.add_op(OpDup).unwrap().add_i64(strike_e8).unwrap().add_op(OpGreaterThanOrEqual).unwrap().add_op(OpVerify).unwrap()
            .add_op(OpBlake2b).unwrap().add_op(OpToAltStack).unwrap();
        // verify all but the first signer with abort-on-fail, keeping the hash on the alt stack
        for pk in committee[1..].iter().rev() {
            b.add_op(OpFromAltStack).unwrap().add_op(OpDup).unwrap().add_op(OpToAltStack).unwrap()
                .add_data(pk).unwrap().add_op(OpCheckSigFromStack).unwrap().add_op(OpVerify).unwrap();
        }
        b.add_op(OpFromAltStack).unwrap().add_data(&committee[0]).unwrap().add_op(OpCheckSigFromStack).unwrap();
        b.drain()
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
}

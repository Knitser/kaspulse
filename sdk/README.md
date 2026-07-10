# kaspulse-sdk — integrate the oracle in ~10 lines

Consume kaspulse prices two ways: **verify them off-chain** (never trust the API)
and **gate a Kaspa covenant on-chain** (L1 enforces the oracle condition).

## Off-chain — fetch + verify
```rust
let feed = kaspulse_sdk::fetch("https://api.kaspulse.example", "KAS/USD")?;
match feed.checked_value() {
    Ok(price) => use_it(price),                 // threshold sigs verified locally
    Err(why)  => reject(why),                   // "halted" / "depegged" / "threshold not met"
}
```
`checked_value()` verifies the **threshold of node signatures** over
`blake2b("kaspulse/v2|PAIR|mant|expo|ts|round")` and honors the safety flags
(`halted`, `peg_ok`, `thin`). The price is `mant × 10^expo` — exact at any
magnitude, from BTC to a $3e-9 meme token.

## On-chain — price-gated covenant (feature = "covenant")
```rust
use kaspulse_sdk::covenant::{price_gate_redeem, price_bytes};
// release funds only if ≥3 oracle nodes signed AND price ≥ $0.02
let redeem = price_gate_redeem(&committee_pubkeys, 2_000_000);
// P2SH-commit `redeem`; to spend, the sig script pushes:
//   [sig_0, sig_1, sig_2, price_bytes(price_e8)]  (nodes sign blake2b(price_bytes))
```
Kaspa L1 verifies, at spend time: **price ≥ strike** *and* **the threshold of
independent oracle signatures** — via `OpCheckSigFromStack`. No off-chain trust.
This is the exact covenant proven live on TN10 (`consumer_live`).

### Covenant templates you can build from this
- **Price-gated payout** — the example above (options, escrows, conditional pay).
- **Liquidation trigger** — gate on `price ≤ strike` (swap `OpGreaterThanOrEqual`
  → `OpLessThanOrEqual`).
- **Range settle** — two strikes, both checked.

## Feed fields worth honoring
| field | meaning |
|---|---|
| `mant`,`expo` | the signed price = `mant × 10^expo` |
| `threshold` / `signers` / `signatures` | k-of-n; `verify()` checks them |
| `halted` | circuit breaker tripped — don't use |
| `thin` | low-liquidity KRC-20 pool — low confidence |
| `peg_ok:false` | the chain's stablecoin/bridge depegged |
| `freshest_ms` | age of the freshest source (ms) |

## Install
```toml
[dependencies]
kaspulse-sdk = { git = "https://github.com/…/kaspulse", package = "kaspulse-sdk", features = ["covenant"] }
```
Off-chain verify needs no features; the covenant builder pulls the Kaspa script engine.

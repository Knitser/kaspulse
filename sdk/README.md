# kaspulse-sdk — integrate the oracle in ~10 lines

Consume kaspulse prices two ways: **verify them off-chain** (never trust the API)
and **gate a Kaspa covenant on-chain** (L1 enforces the oracle condition).

## 1. Quickstart — fetch + verify

```rust
use std::time::Duration;

// public default: https://pulse.kascov.io — override for local dev
let base = "https://pulse.kascov.io";

let feed = kaspulse_sdk::fetch(base, "KAS/USD")?;      // /v1/feed/KAS-USD
let committee = kaspulse_sdk::fetch_committee(base)?;  // /v1/committee pin
feed.verify_with_committee(&committee)?;
let _pe8 = feed.verify_covenant()?;                    // hosted blake2b(price_bytes) sigs
match feed.checked_value_fresh(Duration::from_secs(30)) {
    Ok(price) => use_it(price),   // sigs verified + message fields bound + <30s old
    Err(why)  => reject(why),     // "halted" / "depegged" / "threshold not met" /
}                                 // "message/field mismatch" / "stale"
```

`checked_value_fresh()` verifies the **threshold of node signatures** over
`blake2b("kaspulse/v2|PAIR|mant|expo|ts|round")`, **binds** the signed message's
`PAIR|mant|expo|ts` fields to the JSON fields (so a lying server can't serve a
price the signatures don't cover), honors the safety flags (`halted`, `peg_ok`),
and requires the *signed* timestamp to be fresh. The price is `mant × 10^expo` —
exact at any magnitude, from BTC to a $3e-9 meme token. Pin keys with
`fetch_committee` + `verify_with_committee`. On-chain consumers should also
call `verify_covenant()` for the hosted `blake2b(price_bytes)` domain.

Unknown pair? `fetch` returns `Error::NoSuchFeed` (the oracle answers a real
HTTP 404). For dashboards, `fetch_catalog(base)` polls the light `/v1/feeds`
catalog — one small row per pair instead of the full envelope. Catalog rows are
**not signed**; verify a pair's full feed before acting on it.

## 2. Covenant recipes (feature = "covenant")

Every recipe is the exact script proven live on Kaspa TN10 (`consumer_live`,
`slash_live`) — the SDK ships proven bytes only. To spend, the witness is
`[sig_0..sig_{n-1}, price_bytes, redeem]` bottom→top; the committee signs
`schnorr(blake2b(price_bytes(price_e8)))`.

```rust
use kaspulse_sdk::covenant::{self, Gate, Prefix};

// gate ≥ — release only if the committee signed AND price ≥ $0.02
let redeem = covenant::price_gate_redeem(&committee, 2_000_000);
let addr   = covenant::p2sh_address(&redeem, Prefix::Testnet)?;   // fund this
let witness = covenant::price_gate_witness(&sigs, price_e8, &redeem); // spend with this

// liquidation ≤ — same script, flipped comparison
let redeem = covenant::price_gate_redeem_dir(&committee, 2_000_000, Gate::AtOrBelow);

// range settle — release only if $0.01 ≤ price ≤ $0.03
let redeem = covenant::range_settle_redeem(&committee, 1_000_000, 3_000_000);

// bond + slash — a node that double-signs a (pair, round) slot loses its bond
use kaspulse_sdk::covenant::bond;
let rec1 = bond::attestation_record("KAS/USD", 42, 2_900_000);
let rec2 = bond::attestation_record("KAS/USD", 42, 5_800_000);
assert!(bond::is_equivocation(&rec1, &rec2));
let redeem  = bond::bond_redeem(&node_pk);
let witness = bond::slash_witness(&rec1, &sig1, &rec2, &sig2, &redeem);
```

No reclaim-timelock bond branch is exposed: it's marked unproven in `slash.rs`,
and this SDK ships proven script only.

Runnable, fully-offline examples:
`cargo run -p kaspulse-sdk --example threshold_gate --features covenant` and
`--example catch_equivocation`. For the stepwise on-chain flow (keys → address
→ deploy → spend on TN10) use the repo's gate CLI:
`cargo run --bin gate --features onchain -- demo --strike 0.02 --value 3`.

## 3. Feed fields

The fields worth honoring (`halted`, `thin`, `peg_ok`, `freshest_ms`, …) are
documented once, canonically, on the site's **#/dev** API reference — see the
field table there rather than a drifting copy here.

## 4. Honest status

The hosted committee signs THIS message string — the pipe-delimited v2 message
(`kaspulse/v2|PAIR|mant|expo|ts|round`), which browsers/clients/this SDK
verify. The on-chain covenant flow verifies signatures over
`blake2b(price_bytes)` — those are produced by a locally-generated demo
committee in the guide, not the hosted committee. On-chain consumers (price
gates, slashing) are proven on Kaspa testnet-10; mainnet publishing is next.
(As of July 2026.)

## 5. Install

```toml
[dependencies]
kaspulse-sdk = { git = "https://github.com/Knitser/kaspulse", package = "kaspulse-sdk", features = ["covenant"] }
```

Off-chain verify needs no features; the covenant builder pulls the Kaspa script
engine.

**crates.io: pending.** Two blockers, checked 2026-07-18: (1) crates.io rejects
git dependencies, and the `covenant` feature pins a git rev of `kaspa-txscript`
(the covenants fork); (2) the registry-published `kaspa-txscript` is 0.15.0
(2024-09-27), which predates the Toccata covenants work — it does not expose
`OpCheckSigFromStack` / `TX_VERSION_TOCCATA` (verified present in the pinned
rev `98a4ccd`). The default (off-chain verify) surface has no git deps and is
publishable as-is; the covenant feature can follow once a covenants-capable
`kaspa-txscript` lands on crates.io.

## 6. Changelog

### 0.2.0 (2026-07-18)
- **`verify()` is stricter (breaking in behavior):** it now parses the signed
  message (`Feed::signed_message()`) and requires its `PAIR|mant|expo|ts`
  fields to EQUAL the JSON's `pair`/`mant`/`expo`/`signed_ts`. Previously a
  server could serve a `mant`/`expo` the signatures didn't cover and
  `checked_value()` would return it. It can't anymore.
- New: `Feed::checked_value_fresh(max_age)` — verify + signed-timestamp
  freshness. Use it in anything that moves money.
- **Typed fetch errors (breaking in signature):** `fetch`/`fetch_all` now
  return `Result<_, kaspulse_sdk::Error>` (`NoSuchFeed`/`Http`/`Parse`)
  instead of `Result<_, String>`. Unknown pair is a real 404 →
  `Error::NoSuchFeed`.
- Paths moved to `/v1` (`/v1/feed/{PAIR}`, `/v1/feed`); new `fetch_catalog`
  for the `/v1/feeds` light catalog.
- Covenant module grown from the proven TN10 bins: `Gate` /
  `price_gate_redeem_dir` (≤ gates), `range_settle_redeem`,
  `price_gate_witness`, `p2sh_script` / `p2sh_address`, and `covenant::bond`
  (`attestation_record`, `bond_redeem`, `slash_witness`, `is_equivocation`).
  `price_gate_redeem` output is byte-identical to 0.1.0 (regression-tested).

### 0.1.0
- Initial release: `fetch`/`fetch_all`, `Feed::verify`/`checked_value`,
  `covenant::price_gate_redeem`/`price_bytes`.

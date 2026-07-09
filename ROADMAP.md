# kaspulse — the big plan

**Thesis:** don't fight Chainlink/Pyth on Ethereum (their moat is trust + 2,400
integrations, not speed). **Own Kaspa** — where there's no oracle incumbent — by
being (1) the **fastest** on-chain feed and (2) the only one that prices **KRC-20
tokens**, the Kaspa-native assets the big oracles ignore.

Three edges, all real: **native to Kaspa · sub-second fresh · KRC-20 coverage.**

---

## Where we are (done ✅)
- Median of 6 exchanges → 5-node **3-of-5 threshold** signatures → dashboard + API.
- Independent `verify` tool (no-trust proof).
- **On-chain, live on TN10:** signed price in a tx payload + a consumer covenant
  that releases funds when Kaspa L1 verifies the oracle sig *and* `price ≥ strike`.

Honest gaps: single-process (not real operators), 2s REST polling (not fast),
one asset (KAS/USD).

---

## Phase 1 — Multi-asset + the KRC-20 wedge  ← **building now**
- **Majors** (KAS, BTC, ETH): 5-exchange median each. Trivial — same pipeline.
- **KRC-20** (NACHO, KASPY, …): the differentiator. v1 via market APIs; **real
  version reads Kaspa DEX pool reserves on-chain** (KAS/TOKEN pool → price in KAS
  → USD via our KAS/USD feed). Nobody else does this.
- Dashboard shows a board of feeds; API serves `/api/feed/<pair>`.
- **Why first:** turns "a KAS demo" into "the oracle Kaspa DeFi actually needs,"
  and makes Phases 3–4 possible.

## Phase 2 — Fast af (sub-second)
- Replace REST polling with **WebSocket streams** (Kraken/KuCoin/Bybit push every
  tick, no key) + parallel fetch. → sub-second medians.
- Publish on-chain on **deviation + heartbeat** (not every tick — cost control),
  exploiting Kaspa's ~100ms finality for the freshest on-chain feed anywhere.
- Confidence intervals + staleness/circuit-breakers (don't publish a bad tick).

## Phase 3 — Real on-chain feeds (not one-off)
- A **standing price coin per asset** with a persistent covenant ID (KIP-20),
  updated each round — a contract can always read "the latest KAS/USD."
- A tiny **consumer SDK / covenant template**: 5 lines to gate a contract on a
  kaspulse price (the CSFS pattern we proved, packaged).

## Phase 4 — Land one integration (the moat)
- Package: landing page + a 1-page "integrate kaspulse" guide + live feeds page.
- Take it to **Zealous Swap / DEX.cc** — offer free integration for KRC-20 +
  majors. **One real user beats ten features.** Demand is the moat.

## Phase 5 — Decentralize for real (trust)
- Independent operators on separate machines; k-of-n aggregation over the wire.
- **Staking + slashing**: operators post KAS bond, lose it for bad/late data.
- Only *after* there's demand pulling for it — trust matters once real value rides on it.

## Phase 6 — Sustain it
- Fee model (consumers pay per read / subscription) or a small protocol token for
  operator rewards + staking. Coverage expands to every KRC-20 + forex/commodities.

---

## The one-line sequence
**multi-asset + KRC-20 (now) → fast (WS) → standing feeds + SDK → land a DEX →
decentralize + stake → monetize.** Trust and one real user are the goal; speed
and coverage are the weapons.

*Status: demo/PoC. Real prices, real threshold sigs, real on-chain consumer —
but single-operator until Phase 5. Don't secure real value with it yet.*

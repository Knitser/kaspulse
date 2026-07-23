# kaspulse — the big plan

**Thesis:** don't fight Chainlink/Pyth on Ethereum (their moat is trust + 2,400
integrations, not speed). **Own Kaspa** — where there's no oracle incumbent — by
being (1) the **fastest** on-chain feed and (2) the only one that prices **KRC-20
tokens**, the Kaspa-native assets the big oracles ignore.

Three edges, all real: **native to Kaspa · sub-second fresh · KRC-20 coverage.**

---

## Where we are (done)

- Multi-asset majors (KAS/BTC/ETH) via WebSocket + REST, ~56 KRC-20 from
  Kasplex/Igra/KaspaCom pools with auto-discovery → ~59 feeds.
- MAD outliers, circuit breakers, peg check, thin flags, mant×10^expo v2.
- Hosted dual attestation: `kaspulse/v2|…` **and** `blake2b(price_bytes)` +
  bond records under `feed.covenant`; pin via `/v1/committee`.
- Independent `verify` (SDK-parity field binding) + `kaspulse-sdk` + JS/Python.
- Standalone `signer` + oracle aggregator (`KASPULSE_OPERATORS`).
- **On-chain TN10:** threshold consumer, equivocation slash, standing publisher
  (`standing` bin — deviation + heartbeat + merkle root).
- Live at **https://pulse.kascov.io**.

Honest remaining gaps: hosted keys still co-located until operators are wired
in production; mainnet standing coins not yet; one DeFi integration not landed.

---

## Phase 1 — Multi-asset + the KRC-20 wedge  ← **done**

## Phase 2 — Fast af (sub-second)  ← **done** (WS majors; REST still adds venues)

## Phase 3 — Real on-chain feeds  ← **building now**
- Standing publisher on TN10 (`standing` bin); mainnet + KIP-20 persistent
  covenant id next.
- Consumer SDK shipped; hosted covenant sigs unlock production gates.

## Phase 4 — Land one integration (the moat)
- Target: **Kaspa Finance** (lending needs an oracle), then Zealous.
- Package: `/guide.html` + INTEGRATION.md + live feeds.

## Phase 5 — Decentralize for real (trust)
- Wire multi-host `signer` daemons into live `KASPULSE_OPERATORS` aggregation.
- Bond reclaim timelock now in SDK; prove live reclaim on TN10.
- Community operators after one real consumer.

## Phase 6 — Sustain it
- Fee model / operator rewards. Coverage expands.

---

## The one-line sequence
**multi-asset + KRC-20 → fast (WS) → standing feeds + SDK → land a DEX →
decentralize + stake → monetize.**

*Status: real prices, real threshold sigs, real on-chain consumers on TN10,
hosted covenant dual-sign shipped. Don't secure mainnet value until multi-host
operators + mainnet standing feeds are live.*

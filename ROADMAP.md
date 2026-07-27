# kaspulse — the big plan

**Thesis:** don't fight Chainlink/Pyth on Ethereum (their moat is trust + 2,400
integrations, not speed). Own the part of Kaspa nobody is serving — and be
precise about which part that is, because one of the seats is already occupied.

**The EVM-L2 oracle seat is TAKEN.** Kaskad's COB Oracle has been live on Igra
mainnet since ~May 2026: an AWS Nitro enclave consolidating 15+ exchange order
books, PCR0 attestation verified on-chain, ~30 s cadence, Sherlock-audited
(Apr 2026), KEF-funded, shipped as a Chainlink `AggregatorV3` wrapper that drops
into Aave-style config. If someone needs an audited KAS mark for a lending
market on Igra today, that is the correct answer and we should say so.

**Two seats are open:**
1. **The L1-covenant seat** — a price a Kaspa coin can enforce *itself* at spend
   time, via `OpCheckSigFromStack`. No TEE-in-an-EVM design addresses this;
   it is not a feature they haven't built, it is a different chain layer.
2. **The KRC-20 seat** — the Kaspa-native long tail. Neither COB nor QUEX
   prices a single KRC-20 token, and pricing them honestly (depth in dollars,
   pool touch age, single-venue disclosure) is work nobody else is doing.

Three edges, all real: **native to Kaspa · sub-second majors · KRC-20 coverage.**
"Sub-second" is the majors only — they stream over exchange WebSockets
(`freshest_ms` in the tens of ms). KRC-20 is a ~5 s RPC poll of pool STATE, and
`pool_age_s` says how long since those reserves actually moved, which is
routinely days. Two products, two latencies, said out loud.
One edge we do NOT have: an audit, funding, or a production consumer.

---

## Where we are (done)

- Multi-asset majors (KAS/BTC/ETH) via WebSocket + REST, ~56 KRC-20 from
  Kasplex/Igra/KaspaCom pools with auto-discovery → ~59 feeds.
- MAD outliers, circuit breakers, peg check, thin flags, mant×10^expo v2.
- Hosted attestation: `kaspulse/v2|…`, pin via `/v1/committee`. Bond records
  ship under `feed.covenant`. The `blake2b(price_bytes)` covenant domain was
  **withdrawn 2026-07-27** — its preimage bound a bare integer with no pair,
  expo, round or ts, so one feed's signature satisfied gates on every other
  pair (docs/MESSAGE-FORMAT.md §8.0). The bound `cov/v2` preimage is designed,
  not shipped.
- Independent `verify` (SDK-parity field binding) + `kaspulse-sdk` + JS/Python.
- Standalone `signer` + oracle aggregator (`KASPULSE_OPERATORS`).
- **On-chain TN10:** threshold consumer (demo committee), equivocation slash,
  standing publisher (`standing` bin — deviation + heartbeat + merkle root).
  The bond record widened to 32 bytes on 2026-07-27 (expo is now inside the
  compared tail), which is a new script and a new P2SH — that change is proven
  on the script VM and **not yet re-run on TN10**.
- Live at **https://pulse.kascov.io**.

Honest remaining gaps: hosted keys still co-located until operators are wired
in production; mainnet standing coins not yet; one DeFi integration not landed.

---

## Phase 1 — Multi-asset + the KRC-20 wedge  ← **done**

## Phase 2 — Fast af (sub-second)  ← **done for majors** (WS; REST still adds
venues). Not KRC-20 — those are a ~5 s pool-state poll and always will be.

## Phase 3 — Real on-chain feeds  ← **building now**
- Standing publisher on TN10 (`standing` bin); mainnet + KIP-20 persistent
  covenant id next.
- Consumer SDK shipped. Production gates need the **bound `cov/v2` preimage**
  (§8.0) built, script-rebuilt with `OpSubstr`, and re-proven on TN10 — until
  then the only covenant committee is the guide's local demo one.

## Phase 4 — Land one integration (the moat)
- Target: **Kaspa Finance** (lending needs an oracle), then Zealous. Note the
  honest odds: Kaskad, the one live lending market, built its own oracle
  in-house — a vertically integrated incumbent is the market we are least
  likely to win. The realistic opening is that their docs list no oracle
  fallback, and that KRC-20 collateral has no mark at all.
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

*Status: real prices, real threshold sigs, real on-chain consumers on TN10. The
hosted covenant domain is withdrawn pending `cov/v2`. Don't secure mainnet value
until multi-host operators + mainnet standing feeds are live.*

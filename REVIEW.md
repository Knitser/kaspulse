# kaspulse — thorough improvement review
*2026-07-10 · combines: a fresh code audit, the oracle-reality-check verdict, the
kasphinx ZK toolchain, kascov's infrastructure, and the ecosystem research.*

## 0. Where we stand (one paragraph, honest)
kaspulse is a working single-operator oracle **mechanism**: 59 feeds (3 majors
sub-second via WebSocket, 56 KRC-20 read directly from Kasplex+Igra DEX pools),
median-aggregated, 3-of-5 threshold-signed, consumable off-chain (API) and
on-chain (CSFS covenant, proven on TN10). The reality-check verdict stands: the
architecture is right; what's missing is *independent operators, economic
security, and hardened data integrity*. This review lists every improvement
worth making, sorted by layer, each tagged **[bug] [build] [infra] [social]**.

---

## 1. Code-level defects found (fix first — they're cheap and real)

### P0 — the signed price of tiny tokens is wrong **[bug]**
`price_e8` quantizes to 8 decimals. Measured live: **BONKEY signs price_e8=0
(100% error)**, yoni 26.5% error, PLUTO 3.3%. The *signed, on-chain-consumable*
number is broken for sub-1e-7 tokens.
**Fix:** sign a scaled integer + exponent: `price_scaled × 10^expo` where expo
is per-feed (e.g. mantissa ~9 digits). Message becomes
`kaspulse/v1|PAIR|mantissa|expo|ts|round`. Consumers compare against a strike
at the same expo — same CSFS script shape.

### P1 — `kas_usd()` ignores staleness **[bug]**
It medians *all* book entries regardless of age; a source that froze an hour
ago still votes on the KAS/USD used to price every KRC-20 token.
**Fix:** filter by `age < STALE_MS` (same rule build() already applies).

### P1 — liquidity high-water mark never decays **[bug]**
`liq_map` keeps the max liquidity ever seen; a pool that drains after being
liquid stays unflagged (`thin=false`) forever.
**Fix:** store per-(pair,chain) current values; recompute the max each round.

### P2 — 737 Schnorr signs/sec, mostly redundant **[build]**
59 feeds × 5 keys × 2.5 rounds/s, even when prices didn't change.
**Fix:** sign only on change (deviation) or heartbeat (e.g. 5s). Also the
foundation for on-chain publish cadence (§5).

### P2 — misc
- WS reconnect has no backoff/jitter (3s constant) — fine until an exchange
  rate-limits reconnect storms.
- The built-in HTTP server is fine locally but not for public hosting (no
  limits); public deploy should ride kascov's worker pattern (§7).
- `verify` re-derives the CEX median but not the pool reads — extend it to
  re-run `getReserves` per KRC-20 feed so *every* feed is reproducible.

---

## 2. Data layer — sources

- **All-WebSocket majors [build]:** KuCoin/Gate/MEXC all have public WS tickers;
  move them off 5s REST → every major source sub-second. Then the median itself
  (not just the freshest source) is sub-second.
- **More venues per major [build]:** Binance/OKX/Coinbase(BTC,ETH) public WS —
  7-9 sources per major. Median over 9 tolerates 4 bad venues.
- **KRC-20 venue coverage [build]:**
  - **KaspaCom DEX** — factory spotted at `0x21350BcDa9E81731CF4cDE3DbC457e3de2739c01`
    (Igra explorer, verified contract). If it's v2-shaped, it's a same-day add.
  - **Kaspa Finance (V3)** — needs a `slot0()`/`sqrtPriceX96` reader + tick math.
    A real chunk of work; do it when its TVL justifies it. It's also the natural
    first *consumer* (they plan lending).
- **Auto-discovery [build]:** re-enumerate factories every ~10 min in a
  background thread: new tokens appear, drained pools drop. Removes the static
  `pools.json` snapshot (keep it as a cache).
- **WKAS/WiKAS peg check [build, important]:** every KRC-20 price assumes
  1 WKAS = 1 KAS. If a bridge depegs, all token prices silently go wrong.
  We already read Igra's iKAS/USDC pool → implied iKAS/USD; compare with the
  CEX KAS/USD each round; if |depeg| > X%, flag every feed on that chain
  (`peg_ok:false`) and stop publishing them on-chain.

## 3. Aggregation & manipulation resistance

- **Use the pools' built-in cumulative TWAP [build, high-value]:** confirmed
  live: `price0CumulativeLast` is present on the Zealous pools. Reading the
  cumulator at two block timestamps gives the *true chain-computed* TWAP —
  strictly stronger than our sampled 60s window (which can in principle be
  gamed between samples). Our sampled TWAP stays as the cross-check.
- **Outlier rejection (MAD filter) [build]:** drop sources > k·MAD from the
  median before aggregating; a hijacked venue then contributes nothing at all.
- **Confidence intervals (Pyth-style) [build]:** publish ±interval derived from
  source spread + liquidity depth; consumers can require `conf < X`.
- **Liquidity-weighted cross-venue price [build]:** NACHO is 1M WiKAS deep on
  Igra vs 7.9 WKAS on Kasplex — a plain median across the two venues weights
  them equally, which is wrong. Weight by (or select on) liquidity.
- **Circuit breakers [build]:** max per-round jump (e.g. >20% → hold last good
  value + flag `halted`), min-sources rule per feed, global sanity bounds.

## 4. Trust — decentralization & economic security (the verdict's fatal gaps)

- **Real operator separation [infra+social]:** extract a tiny `signer` daemon
  (fetch → median → sign → POST to the aggregator). Run the 5 keys on
  genuinely separate infra as step 1 (e.g. Cloud Run + the Mac + a VPS…), then
  recruit community operators — Kaspa has an active node-runner culture; the
  oracle operator set is a natural extension. Until k>1 *people*, the threshold
  is decoration (verdict's words).
- **Equivocation slashing — buildable TODAY on Kaspa [build, novel]:**
  double-signing is *cryptographically provable*: two valid Schnorr sigs by the
  same node key over two different prices for the same `(pair, round)`.
  A **bond covenant** per operator can make this self-enforcing on L1:
  sig-script supplies both signed messages + both signatures; the script
  verifies both via `OpCheckSigFromStack`, checks the two messages share
  `(pair, round)` but differ in price (substring introspection, KIP-17), and
  releases the bond to the whistleblower. **Staking + slashing with no
  governance, no committee — pure script.** Nobody has done this on Kaspa; it
  directly answers the "economic security" gap with our own covenant toolchain.
- **ZK-verified aggregation (kasphinx × kaspulse) [build, research-grade]:**
  we already generate Groth16 proofs Kaspa's KIP-16 precompile accepts. A
  circuit proving "published median = median(these N signed source prices)"
  makes even the *aggregation* trustless — a zkOracle. Real differentiator,
  honest label: research project, not this month.

## 5. On-chain delivery (Kaspa L1)

- **Standing price coins [build]:** one covenant per feed with a **KIP-20
  covenant id** as its persistent identity; update on deviation (>0.5%) +
  heartbeat (60s). Consumers reference "the latest state of covenant X" —
  a real feed, not a one-off payload. Our TN10 key (199k TKAS) funds this today.
- **One tx, all feeds — merkle root [build]:** publishing 59 feeds separately
  is wasteful. Publish `root = merkle(all pair prices)` signed once per round;
  a consumer supplies the merkle path in its sig-script. 59 feeds for the cost
  of one update.
- **Threshold on-chain, not 1-of-1 [build]:** the TN10 consumer covenant
  verifies ONE oracle key. Make the redeem require 3 CSFS checks against 3 of
  the 5 published node keys — the off-chain threshold enforced on-chain.
- **Consumer SDK [build]:** a `kaspulse-consumer` crate + covenant templates
  (price-gate, range-settle, liquidation-trigger) + 1-page integration doc.
  This is what a DEX/lending team actually needs to adopt it.

## 6. Speed

- Event-driven signing: sign on tick arrival, not a 400ms loop → end-to-end
  (exchange tick → signed feed) ~50-150ms for majors.
- SSE/WebSocket push API for consumers (kascov already ships an SSE pattern we
  can lift verbatim).
- Honest ceiling: KRC-20 freshness is bounded by chain block time + RPC; ~1-2s
  there is fine and honest.

## 7. Infra — combine with what we already run (kascov)

- **Own nodes [infra]:** the roadmap's trust fix — run our own **Kasplex node +
  Igra node** (both EVM; modest boxes) and set `KASPLEX_RPCS`/`IGRA_RPCS` to
  ours+public (the cross-check we built activates: any single RPC lying =
  read dropped). A **Kaspa L1 node** (utxoindex) for first-party publish
  confirmation. This closes the "public RPC in the trust path" hole.
- **Deploy like kascov [infra]:** oracle → Cloud Run (kascov's worker pattern,
  budget-guarded); dashboard → Firebase Hosting (`kaspulse.web.app` or a
  domain); publish the JSON API publicly. kascov's alerting/monitoring carries
  over nearly unchanged.
- **kascov × kaspulse synergy [build]:** kascov (the covenant explorer) should
  *recognize and name* kaspulse price-coin covenants — feed updates become
  browsable covenant stories; the explorer gets living content, the oracle gets
  a public audit trail. Both sites cross-link.

## 8. Product & adoption (unchanged from ROADMAP, sharpened)

1. Ship §5 (standing feeds + SDK) → something integrable exists.
2. **Kaspa Finance is the first target** (V3 DEX planning lending — lending
   *needs* an oracle; we already price their competitor venues). Zealous next.
3. Publish the dashboard + docs publicly (kascov infra).
4. Operator program (§4) once one consumer is real.

---

## The priority list (what I'd actually do, in order)

| # | Item | Type | Effort |
|---|------|------|--------|
| 1 | P0/P1 bug fixes (§1: e8 quantization, stale kas_usd, liq decay) | bug | hours |
| 2 | Peg check + circuit breakers + MAD outlier filter (§2/§3) | build | ~a day |
| 3 | Cumulative on-chain TWAP (§3) | build | ~a day |
| 4 | All-WS majors + event-driven signing (§2/§6) | build | ~a day |
| 5 | Standing price coins + merkle root + 3-of-5 on-chain (§5) | build | days |
| 6 | Auto-discovery + KaspaCom venue (§2) | build | ~a day |
| 7 | Own Kasplex/Igra nodes + public deploy on kascov infra (§7) | infra | days |
| 8 | Consumer SDK + docs + first-integration outreach (§5/§8) | build+social | days |
| 9 | Equivocation-slashing bond covenant (§4) | build, novel | ~week |
| 10 | Real operator separation → community operators (§4) | social | ongoing |
| 11 | zkOracle aggregation proof (§4, kasphinx) | research | weeks |

**The one-sentence take:** fix the signed-price bug today, harden the data
(peg/outliers/TWAP) this week, ship standing on-chain feeds + SDK next, and
pursue the two things nobody on Kaspa has — **script-enforced slashing** and a
**ZK-verified aggregation** — as the moats that turn "first oracle on Kaspa"
into "the oracle Kaspa can actually trust."

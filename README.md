# kaspulse — a real-time price oracle for Kaspa

An oracle brings off-chain data (like the KAS/USD price) **on-chain**, so smart
contracts (Kaspa covenants) can use it. kaspulse is a price oracle built for
Kaspa's post-Toccata L1: every price a median of independent venues — including
the KRC-20 tokens read straight from Kasplex/Igra DEX pools that nobody else
carries — threshold-signed by 5 nodes (3-of-5), served sub-second, and
**verifiable by anyone**: in your browser, in one file of Python or JS, or by a
Kaspa covenant on-chain at spend time. The one idea: **you should never have to
trust the oracle.**

Live at: *(deployed origin — see [DEPLOY.md](DEPLOY.md))*

---

## How it works

```
   exchanges + DEX pools        oracle                     consumers
 ┌────────────────────┐   ┌──────────────────┐        ┌────────────────┐
 │ Kraken   Bybit     │   │ 1. fetch all     │        │ your DEX,      │
 │ OKX      Coinbase  │──▶│ 2. MAD-filtered  │──────▶ │ lending, perps │
 │ KuCoin   Gate.io   │   │    MEDIAN        │ signed │ read the price │
 │ MEXC               │   │ 3. 5 nodes SIGN  │ price  │ + check the    │
 │ Kasplex/Igra pools │   │    it (3-of-5)   │        │ signatures     │
 │  (56 KRC-20, live) │   │ 4. serve /v1     │        │ (off/on-chain) │
 └────────────────────┘   └──────────────────┘        └────────────────┘
```

1. **Fetch** — majors stream over WebSocket (sub-second); KRC-20 prices are
   read directly from DEX pool reserves on-chain, cross-checked across RPCs.
2. **Aggregate** — the **median**, behind a MAD outlier filter, circuit
   breakers, a WKAS peg check, and thin-pool flags. No single venue can move
   the feed; a bad tick gets held, not published.
3. **Sign** — 5 nodes, each with its own key, sign
   `kaspulse/v2|PAIR|mant|expo|ts|round`. A consumer needs **3-of-5** —
   no single node can forge a price.
4. **Serve** — a live dashboard + a JSON API (`/v1/feed`), ~59 feeds at last
   count (3 majors + 56 KRC-20; discovery re-enumerates the DEX factories
   every 10 minutes, so the count moves).
5. **Consume on-chain** — a Kaspa covenant verifies the signatures itself with
   `OpCheckSigFromStack` and releases funds only if the price clears a strike —
   proven on testnet-10 (see the proof table below).

The full pipeline, thread map and build features:
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). Why Kaspa is the right chain for
this (~100 ms blocks, no leader to front-run an oracle update):
[KASPA-EDGE.md](KASPA-EDGE.md).

---

## Status, honestly

*This is the one canonical status block — every other kaspulse page links here
instead of restating it:*

- **The oracle, the signatures and every number it serves are live and real**
  — verify one yourself: the site's verify button, `clients/py/kaspulse.py`,
  or `cargo run --bin verify`. Live at https://pulse.kascov.io.
- **On-chain consumers are proven on Kaspa testnet-10** — threshold
  price-gated spends and equivocation slashing ran on the real chain (proof
  table below). Standing feeds: `cargo run --bin standing --features onchain`
  (deviation + heartbeat). **Mainnet publishing is next.**
- **The hosted committee's 5 keys currently sign in one process** by default.
  Set `KASPULSE_OPERATORS` to poll independent `signer` `/attest` endpoints
  ([docs/OPERATOR.md](docs/OPERATOR.md)); cryptography is real 3-of-5.
- **Hosted dual attestation shipped.** Each feed carries `covenant.signatures`
  over `blake2b(price_bytes)` plus bond `record` sigs; pin keys via
  `/v1/committee` + `Feed::verify_with_committee`. The guide's local demo
  committee remains the offline walkthrough.
- Not financial infrastructure yet — don't secure real value with it until
  multi-host operators + mainnet standing feeds are live.

---

## Proof — on the real chain, reproducible

No txids are pinned in this repo (testnet runs are ephemeral); each row links
the bin that reproduces the result end-to-end on testnet-10. You need a funded
TN10 key at `~/.kaspulse/tn10.key` (faucet) — see
[/guide.html](web/guide.html) for the walkthrough with fresh txids.

| claim | reproduce with |
|---|---|
| **Threshold consumer spend** — a covenant releases funds only when 3 independent node keys signed the price AND price ≥ strike, verified by L1 script | `cargo run --bin consumer_live --features onchain` |
| **Equivocation self-slash** — a bond coin slashed on-chain with a real double-signing proof; honest updates / re-signs / forged sigs correctly NOT slashable | `cargo run --bin slash_live --features onchain` (script-engine proof of all 4 cases: `--bin slash`) |
| **1.39 s measured tick→UTXO latency** — avg of 3 live rounds (1510/1154/1507 ms) via a *public* node; own-node path is sub-second ([KASPA-EDGE.md](KASPA-EDGE.md)) | `cargo run --bin latency --features onchain` |

---

## 30-second quickstart

Run the oracle (needs Rust):

```sh
cargo run --bin oracle          # dashboard + API on http://localhost:8080
```

Read a signed price, then **verify it without trusting us** (Python 3, no
dependencies — the client is one file):

```sh
curl -s http://localhost:8080/v1/feed/KAS-USD
python3 clients/py/kaspulse.py verify KAS/USD http://localhost:8080
```

The verify step re-checks all 5 BIP340 signatures over the signed message and
binds the message fields to the JSON — per-node ✓/✗, threshold verdict, done.
For maximum paranoia, `cargo run --bin verify` also re-fetches the exchanges
and recomputes the median itself.

---

## The API (v1, frozen)

`GET /v1/feed` (full envelope) · `GET /v1/feed/{PAIR}` (one feed, dash form:
`KAS-USD`; unknown pair → 404) · `GET /v1/feeds` (light catalog — what
dashboards should poll) · `GET /health`. Legacy aliases `/api/feed`,
`/api/feed/{PAIR}` and `/feed.json` are permanent. CORS `*`, no auth, no keys.

The **signed price is `mant × 10^expo`** (9 significant digits at any
magnitude — a $3e-9 meme token signs as precisely as BTC). Message format:
`kaspulse/v2|PAIR|mant|expo|ts|round`; verification is BIP340 over the
blake2b-256 of that string, **plus the field-binding check** — the normative
spec with test vectors is [docs/MESSAGE-FORMAT.md](docs/MESSAGE-FORMAT.md),
and the full endpoint reference with live examples is on the site under
`#/dev`. Unchanged prices re-sign on a 5 s heartbeat; changed prices sign
immediately (`signed_ts` tells you which attestation you hold).

---

## Repo map

| path | what |
|---|---|
| `src/main.rs` | the `oracle` bin — fetch → median → sign → serve |
| `src/http.rs` | the std-threads HTTP server (v1 API, static site, share/OG) |
| `src/verify.rs` | `verify` — the no-trust auditor (re-checks sigs, re-fetches exchanges) |
| `src/signer.rs` | `signer` — standalone independent-operator daemon ([docs/OPERATOR.md](docs/OPERATOR.md)) |
| `src/gate.rs` | `gate` — covenant CLI: `keygen` / `address` / `deploy` / `spend` / `demo` (feature `onchain`) |
| `src/consumer_live.rs`, `src/slash.rs`, `src/slash_live.rs`, `src/latency.rs`, `src/onchain.rs` | the on-chain proofs (feature `onchain`) — see the proof table |
| `sdk/` | `kaspulse-sdk` — verified fetch + covenant builders ([sdk/README.md](sdk/README.md)) |
| `clients/` | zero-dependency verifying clients: `js/kaspulse.mjs`, `py/kaspulse.py` |
| `web/` | the dashboard SPA + [`guide.html`](web/guide.html) (the 15-minute covenant walkthrough) |
| `docs/` | [MESSAGE-FORMAT](docs/MESSAGE-FORMAT.md) · [ARCHITECTURE](docs/ARCHITECTURE.md) · [OPERATOR](docs/OPERATOR.md) · [INTEGRATION](docs/INTEGRATION.md) |
| `scripts/` | `deploy.sh`, `setup-keys.sh`, `check-vendored.sh` |

Integrating? Start at [docs/INTEGRATION.md](docs/INTEGRATION.md) — a one-page
decision tree (read off-chain / gate on-chain / audit us / run a signer).

---

## Docs & links

- [/guide.html](web/guide.html) — an oracle-gated covenant on testnet-10 in 15 minutes
- site `#/dev` — the API reference with live examples
- [docs/MESSAGE-FORMAT.md](docs/MESSAGE-FORMAT.md) — the normative v2 interop spec (write your own verifier)
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — the pipeline, threads, features
- [docs/OPERATOR.md](docs/OPERATOR.md) — run a signer; what gets slashed
- [docs/INTEGRATION.md](docs/INTEGRATION.md) — pick your integration branch
- [DEPLOY.md](DEPLOY.md) — hosting, keys, own-node config
- [KASPA-EDGE.md](KASPA-EDGE.md) — why Kaspa, with the measured latency numbers
- [ROADMAP.md](ROADMAP.md) · [REVIEW.md](REVIEW.md) — the plan and the audit trail

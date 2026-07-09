# kaspulse — a real-time price oracle for Kaspa

An oracle brings off-chain data (like the KAS/USD price) **on-chain**, so smart
contracts (Kaspa covenants) can use it. kaspulse is the first price oracle built
for Kaspa's post-Toccata L1.

The one idea: **you should never have to trust the oracle.** Every price is a
median of independent exchanges, signed by a threshold of independent nodes, and
verifiable by anyone — including on-chain by a Kaspa script.

---

## How it works

```
   exchanges                oracle                     consumers
 ┌───────────┐        ┌──────────────────┐        ┌────────────────┐
 │ Kraken    │        │ 1. fetch all     │        │ your DEX,      │
 │ KuCoin    │──────▶ │ 2. take the      │──────▶ │ lending, perps │
 │ Gate.io   │  live  │    MEDIAN        │ signed │ read the price │
 │ Bybit     │ prices │ 3. each node     │ price  │ + check the    │
 │ MEXC      │        │    SIGNS it      │        │ signatures     │
 │ CoinGecko │        │ 4. serve + publish│        │ (off/on-chain) │
 └───────────┘        └──────────────────┘        └────────────────┘
```

1. **Fetch** — pull KAS/USD from 6 independent venues. If one is down or lying,
   the median ignores it.
2. **Aggregate** — the **median** price. No single exchange can move the feed.
3. **Sign** — **5 independent nodes**, each with its own key, sign the price. A
   consumer needs a **threshold (3-of-5)** to accept it — no single node (not
   even the operator) can forge a price.
4. **Serve** — a live dashboard + a JSON API (`/api/feed`).
5. **Publish on-chain** — the signed price goes into a Kaspa covenant "price
   coin"; a consumer covenant verifies the signatures on-chain with
   `OpCheckSigFromStack` (KIP-16/covenants). *(in progress — see Roadmap)*

Why Kaspa: its **~100ms blocks** let a price land and finalize on-chain faster
than almost any other L1 — the freshness perps/liquidations need.

---

## What's built (and what isn't — honestly)

| Piece | Status |
|---|---|
| Fetch median from 6 exchanges | ✅ working |
| Threshold signing (5 nodes, 3-of-5) | ✅ working |
| Live dashboard + JSON API | ✅ working |
| Independent verifier (`verify`) | ✅ working |
| On-chain price coin + consumer covenant | 🔨 in progress (`onchain`, feature-gated) |
| Multi-machine operators, staking/slashing | 🔭 design (see below) |

This is a **single-process demo**: the 5 "nodes" run in one binary. In
production each node is a **separate operator on a separate machine**, and they'd
**stake** collateral (slashed for bad data). The cryptography (median + threshold
signatures) is exactly the real thing; the decentralized *deployment* is the
part a demo can't show.

---

## Run it

Needs Rust (`cargo`) and Python 3 (for nothing — the oracle serves itself).

```sh
cargo run --bin oracle
```

That starts the fetch→median→sign loop and serves everything on
**http://localhost:8080** — open it in a browser for the live dashboard.

---

## Test it

**1. Watch it live** — open http://localhost:8080. The price updates every ~2s;
you can see each exchange, the median, the spread, and the signatures.

**2. Read the raw signed feed:**
```sh
curl http://localhost:8080/api/feed
```

**3. Prove it's honest — the important one.** In another terminal:
```sh
cargo run --bin verify
```
This trusts the oracle for *nothing*: it re-checks every node's signature over
the price, and re-fetches the exchanges to recompute the median itself. If both
match, you get: *"honest feed — no trust required."*

You can point it at any feed URL: `cargo run --bin verify -- <url>`.

---

## The API

`GET /api/feed` returns:
```json
{
  "pair": "KAS/USD",
  "price": 0.029123,
  "price_e8": 2912300,
  "round": 131,
  "timestamp": 1783588006,
  "sources": [{"name":"Kraken","price":0.02911}, ...],
  "median": 0.029123,
  "spread_bps": 13.7,
  "signers":  ["<node0 pubkey>", ...5],
  "threshold": 3,
  "signatures": ["<schnorr sig>", ...5],
  "message": "kaspulse/v1|KAS/USD|2912300|1783588006|131",
  "history": [[ts, price], ...]
}
```
A consumer verifies `schnorr_verify(signature, blake2b(message), signer)` for a
threshold of signers.

---

## Roadmap

- **On-chain (`onchain` bin):** publish the signed price into a Kaspa covenant
  price coin, and a consumer covenant that releases a coin only when a threshold
  of oracle signatures verify (via `OpCheckSigFromStack`) **and** the price
  clears a strike — a real price-triggered payment. Build with:
  `cargo run --bin onchain --features onchain`.
- **Decentralize for real:** independent operators on separate machines; k-of-n
  aggregation; **staking + slashing** for economic security.
- **More feeds:** BTC, ETH, every KRC-20 token, forex.
- **Push, not poll:** sub-second updates from first-party sources.

---

*kaspulse is a demo/proof-of-concept. Prices are real; the oracle is a single
operator until decentralized. Not financial infrastructure yet — don't secure
real value with it.*

# Run a kaspulse signer

The oracle's honest gap is stated in the [README](../README.md#status-honestly):
the hosted committee's 5 keys currently sign in one process. The fix is this
daemon — **independent operators, each running `signer` on their own machine,
with their own key, fetching the market themselves.** This page is everything
an operator needs.

*As of July 2026 the operator set is not open enrollment: the hosted committee
doesn't yet aggregate external `/attest` responses into the published feed.
Running a signer today means proving out the operator role (and the three
proven-separate signers came from exactly this bin); joining the live committee
means coordinating with the maintainer. This page will say so when that
changes.*

## 1. Key + start

```sh
cargo run --release --bin signer -- [key_path] [port]   # defaults: signer.key 9099
```

First run with no key file at `key_path` **generates one** and writes it there
(64-char hex secret key). That file *is* your operator identity:

- back it up offline; anyone holding it can sign prices as you
- it's covered by the repo's `.gitignore` (`*.key`) — keep it that way
- the daemon prints your **x-only public key** at startup; that's what you
  hand to the committee and what your (future) bond commits to

## 2. What it does — independence is the point

Every 2 s the signer **fetches its own market view** — Kraken, KuCoin,
Gate.io, Bybit, MEXC via public REST — medians it, and signs
`kaspulse/v2|PAIR|mant|expo|ts|round` for KAS/USD, BTC/USD and ETH/USD
([MESSAGE-FORMAT.md](MESSAGE-FORMAT.md) has the exact format).

It deliberately does **not** fetch prices from the kaspulse API or any other
signer. A threshold of nodes that all read one upstream is decoration; a
threshold of nodes that each measure the market independently is the product.
Don't "optimize" this by pointing your signer at someone else's feed.

## 3. The `/attest` contract

`GET http://<host>:<port>/attest` → `200`, `application/json`,
`Access-Control-Allow-Origin: *`. Body: a JSON **array** (one element per
pair the signer currently prices; a pair with no fetchable price is omitted):

```json
[
  {
    "pair": "KAS/USD",
    "mant": 824000000,
    "expo": -10,
    "ts": 1784380800,
    "round": 4242,
    "signer": "<32-byte x-only pubkey, lowercase hex>",
    "signature": "<64-byte BIP340 sig, lowercase hex>",
    "message": "kaspulse/v2|KAS/USD|824000000|-10|1784380800|4242"
  }
]
```

`signature` is BIP340 over `blake2b-256(message)`; `round` is the signer's own
counter (restarts at 1 on process restart — aggregators key on `(signer, ts)`,
not on cross-operator round agreement). An aggregator polls each operator's
`/attest` and assembles the k-of-n feed; anyone can verify each element with
the field-binding check from the message-format spec.

## 4. systemd unit

```ini
# /etc/systemd/system/kaspulse-signer.service
[Unit]
Description=kaspulse oracle signer
After=network-online.target
Wants=network-online.target

[Service]
User=kaspulse
WorkingDirectory=/home/kaspulse/kaspulse
ExecStart=/home/kaspulse/kaspulse/target/release/signer /home/kaspulse/operator.key 9099
Restart=always
RestartSec=5
# the key file is the identity — keep the unit and dir non-world-readable
UMask=0077

[Install]
WantedBy=multi-user.target
```

```sh
cargo build --release --bin signer
sudo systemctl enable --now kaspulse-signer
curl -s localhost:9099/attest | head -c 400
```

## 5. Monitoring

The daemon logs one line per round (`round N: signed M pairs`). Watch for:

- **`/attest` reachable and fresh** — poll it and alert if the newest `ts` is
  older than ~30 s. An empty array (`[]`) for more than a few rounds means all
  exchange fetches are failing (egress/DNS problem on your box).
- **signed M pairs < 3** — one or more pairs persistently unfetchable.
- **clock skew** — `ts` is your machine's unix time and consumers check
  freshness against it; run NTP. A skewed clock makes your attestations look
  stale (or worse, future-dated).
- restarts (`systemctl status`, `Restart=always` counter) — flapping means
  something's wrong even if it self-heals.

## 6. The bond — what actually gets slashed

Economic security comes from an **equivocation bond**: a coin locked by a
covenant that pays out to *anyone* who proves your key signed two different
prices for the same slot. The mechanics are in
[MESSAGE-FORMAT.md §8.1](MESSAGE-FORMAT.md#81-the-24-byte-attestation-record-equivocation-bond)
(the 24-byte attestation record); the covenant and a real slash on testnet-10
are reproduced by:

```sh
cargo run --bin slash --features onchain        # local script-engine proof, 4 cases
cargo run --bin slash_live --features onchain   # deploys a real bond on TN10 and slashes it
```

`slash_live` is also how a bond is posted: it builds the bond covenant for a
node key and funds it (from `~/.kaspulse/tn10.key` — testnet only, today).

**Exactly what is slashable** — the four cases `slash` proves on the script
engine (`src/slash.rs`):

| case | example | slashed? |
|---|---|---|
| equivocation — same `(pair, round)` slot, two different prices, both validly signed by your key | slot `KAS/USD#42`: $0.029 **and** $0.058 | **yes** — anyone with the two records + signatures takes the bond |
| honest update — different rounds, different prices | `#42` $0.029, then `#43` $0.058 | no |
| re-signing the same price for the same slot | `#42` $0.029, twice | no |
| a second signature forged with a different key | attacker "frames" you | no — the script checks both sigs against *your* pubkey |

In short: **the only slashable act is signing two prices for one slot.** Being
offline, being slow, or disagreeing with the median is not slashable by this
covenant — an honest node that only ever signs what it measured, once per
slot, cannot lose its bond. (The honest-reclaim timelock branch is deliberately
not in the SDK yet — unproven code doesn't ship; until then treat a posted
bond as one-way. Testnet only.)

## 7. Operator checklist

- [ ] `signer` built `--release`, running under systemd, `Restart=always`
- [ ] key file backed up offline, `0600`, never in git
- [ ] x-only pubkey communicated to the committee
- [ ] NTP active; `/attest` freshness monitored (~30 s alert)
- [ ] machine is genuinely yours — not the same box/provider/account as
      another committee member (correlated operators re-create the single
      point of failure this daemon exists to remove)

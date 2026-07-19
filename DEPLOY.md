# Deploying kaspulse

The `oracle` binary serves BOTH the dashboard and the JSON API on `$PORT`
(default 8080, binds `0.0.0.0`), so one container is the whole public service:
one stateful singleton (live exchange WebSockets + the in-memory signing
state) that must run 24/7. Topology: a single Cloud Run service in
`europe-west4`, `--min-instances 1 --max-instances 1` (the max is the budget
guard), on the `*.run.app` URL until a domain is chosen.

## 1. One-time: committee keys → Secret Manager (deploy blocker)

Committee continuity IS the product. A keyless restart mints a brand-new
committee and breaks every verifier and on-chain consumer that pinned the old
pubkeys. So before any public deploy:

```sh
scripts/setup-keys.sh        # pipes kaspulse-node-{0..4}.key into the
                             # kaspulse-node-keys secret, never echoing them
```

The deploy runs with `KASPULSE_REQUIRE_KEYS=1` and the secret injected as
`KASPULSE_NODE_KEYS`; if the keys are missing or malformed the service logs
the exact token `KASPULSE_KEYS_MISSING` and exits 1 instead of silently
minting a fresh committee.

## 2. One-time: publish the GitHub repo (deploy blocker)

The site and docs link `github.com/Knitser/kaspulse` in ~11 places — the
nav/footer, both docs-hub cards, the `#/dev` quickstart `curl`s against
`raw.githubusercontent.com` (the ONLY download path for the zero-dep
clients), and the SDK's git-dependency install snippet. That repo does not
exist yet, so every one of those links 404s until it is published (or the
slug is corrected everywhere).

Before pushing it public: `*.key` is gitignored and no key file is tracked
(verified with `git ls-files`), but re-check — `kaspulse-node-{0..4}.key` sit
in the repo root and must never leave this machine.

## 3. Every deploy

```sh
PROJECT=<gcp-project> scripts/deploy.sh
# optionally: BASE_URL=https://your.domain scripts/deploy.sh
```

Idempotent: enables APIs, ensures the Artifact Registry repo, builds via
Cloud Build (the Dockerfile fetches the OG-card fonts and builds with
`--features og`), deploys, then ensures monitoring (list-before-create — no
duplicate checks). If `BASE_URL` isn't provided it is pointed at the
`*.run.app` service URL after the deploy.

The script never uses `--set-env-vars` (it REPLACES the whole env and once
silently disarmed a key on kascov) — only `--update-env-vars`.

## 4. Env vars

| var | who sets it | meaning |
|---|---|---|
| `PORT` | Cloud Run | listen port (default 8080) |
| `BASE_URL` | deploy.sh (or you) | absolute origin for `/share`, `/og`, `/sitemap.xml`; unset ⇒ sitemap 404s and share pages use a relative og:image path |
| `KASPULSE_NODE_KEYS` | Secret Manager via `--update-secrets` | comma-separated 5×64-hex committee secret keys |
| `KASPULSE_REQUIRE_KEYS` | deploy.sh (`=1`) | refuse to boot without valid keys (log `KASPULSE_KEYS_MISSING`, exit 1) |
| `KASPLEX_RPCS` / `IGRA_RPCS` | you (optional) | comma-separated RPC lists — with ≥2 the cross-check drops any single lying RPC |

## 5. Monitoring

`deploy.sh` ensures (idempotently):

- uptime checks on **`/health`** (not `/healthz` — GFE swallows it on
  `*.run.app`) and **`/v1/feed`**
- log-based alert policies matching the two exact tokens the oracle logs:
  - `KASPULSE_KEYS_MISSING` — boot refused without the committee
  - `KASPULSE_DISCOVERY_EMPTY` — DEX auto-discovery returned a near-empty set

**Manual step:** attach a notification channel to each policy/check in the
console (channels are account-specific) — until then nothing pages you.

`/health` returns `{"ok":…}` with 200/503 — ok means the last build is <5s
old and ≥1 feed is live.

## 6. Local / VPS (systemd)

```sh
cargo build --release --bin oracle          # lean build, no OG cards
scripts/fetch-fonts.sh                      # optional: enables /og cards…
cargo build --release --bin oracle --features og   # …with this build
PORT=8080 ./target/release/oracle
```

Dashboard at `/`, API at `/v1/feed` (legacy `/api/feed` and `/feed.json`
remain forever as aliases).

## Own-node config (removes the last third party)

Set the RPC envs to your own Kasplex/Igra nodes (+ a public one to cross-check):

```sh
KASPLEX_RPCS="https://your-kasplex-node,https://evmrpc.kasplex.org" \
IGRA_RPCS="https://your-igra-node,https://rpc.igralabs.com:8545" \
PORT=8080 ./target/release/oracle
```

With ≥2 RPCs the cross-check activates: any single RPC that lies gets its read
dropped.

## Operators (decentralization)

Each independent operator runs `signer <their-key> <port>` on their own box and
exposes `/attest`; an aggregator polls them for the k-of-n. See `src/signer.rs`.

## Notes / later

- If a CDN is ever put in front, keep API clients pointed at the `run.app`
  origin directly (kascov lesson: CDN buffering vs `no-store` responses).
- Later, not built: publishing the five committee x-only pubkeys as a
  first-class artifact once custody is settled, plus expected-committee
  pinning in the verifiers (`verify_with_committee` in the SDK, an
  `expectedSigners` option in the JS/Python clients and the browser
  verifier) — today every verifier checks against the `signers` array
  carried in the same response, which the site copy now states honestly.
- Later, not built: hosted-committee signatures over `blake2b(price_bytes)`
  (a `covenant` object per feed — the prerequisite for gating on the HOSTED
  committee on-chain; today's covenant guide honestly uses a demo committee),
  an `/api/committee` endpoint, per-IP rate limiting (lift kascov's
  ToolLimiter if `/og` abuse shows in logs), custom-domain mapping.

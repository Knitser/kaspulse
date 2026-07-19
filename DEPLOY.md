# Deploying kaspulse

The `oracle` binary serves BOTH the dashboard and the JSON API on `$PORT`
(default 8080; `KASPULSE_BIND=127.0.0.1` for loopback-only behind a local
proxy), so one process is the whole public service: one stateful singleton
(live exchange WebSockets + the in-memory signing state) that must run 24/7.

## ACTIVE topology (chosen 2026-07-19): VPS + kaspulse.web.app

The oracle runs on a VPS; the public URL is **https://kaspulse.web.app**
(Firebase Hosting site `kaspulse` in `kascov-explorer`, already created).
Hosting can only rewrite to Cloud Run, so a scale-to-zero Caddy proxy
bridges to the VPS:

```
kaspulse.web.app → Cloud Run kaspulse-proxy (min-instances 0, pennies)
                 → https://<ip-dashes>.sslip.io (Caddy on the VPS, free LE TLS)
                 → 127.0.0.1:8080 (oracle, systemd, KASPULSE_REQUIRE_KEYS=1)
```

Three idempotent scripts, run in order from the repo root (the machine
holding the committee key files):

```sh
./deploy/vps/deploy-vps.sh user@host        # build + keys + systemd + caddy
./deploy/proxy/deploy-proxy.sh <sslip-host> # printed by the previous step
./deploy/hosting/deploy-hosting.sh          # points kaspulse.web.app at it
```

Committee keys travel by `scp` in step 1 (never through git); the systemd
unit sets `BASE_URL=https://kaspulse.web.app` so share/OG/sitemap links are
branded. Re-run step 1 to update after a `git push`; re-run step 2 only if
the VPS IP changes.

---

The sections below are the ALTERNATIVE all-Cloud-Run topology (single
service, `--min-instances 1 --max-instances 1` as the budget guard) — kept
for when the oracle outgrows the VPS.

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

## 2. One-time: publish the GitHub repo — DONE 2026-07-19

`github.com/Knitser/kaspulse` is live and public; all ~11 site/docs links
resolve. (Pre-push key check was run: `*.key` gitignored, none tracked, none
anywhere in history.)

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

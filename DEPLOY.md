# Deploying kaspulse

The `oracle` binary serves BOTH the dashboard and the JSON API on `$PORT`
(default 8080; `KASPULSE_BIND=127.0.0.1` for loopback-only behind a local
proxy), so one process is the whole public service: one stateful singleton
(live exchange WebSockets + the in-memory signing state) that must run 24/7.

## ACTIVE topology (chosen 2026-07-20): pulse.kascov.io on the Windows VPS

The oracle runs on the existing **Windows Server VPS (157.90.7.39)** — the
same box that already serves `ironwood.live` behind **Caddy** (auto Let's
Encrypt TLS). kaspulse gets its own subdomain of the domain we already own,
pointed straight at the VPS — no Cloud Run, no Firebase in the path:

```
pulse.kascov.io → A record (Squarespace DNS) → 157.90.7.39
               → Caddy on the VPS (shared with ironwood.live, auto-TLS)
               → 127.0.0.1:<port> (oracle service, KASPULSE_BIND=127.0.0.1,
                 KASPULSE_REQUIRE_KEYS=1, BASE_URL=https://pulse.kascov.io)
```

Two steps to go live:

1. **DNS (Squarespace dashboard):** add an `A` record — Host `pulse`,
   Value `157.90.7.39`. (kascov.io's nameservers are `nsd1–4.squarespacedns.com`.)
2. **VPS:** build the release, run `oracle` as a Windows service (same
   service manager as ironwood.live), drop the five committee key files
   next to it (`scp`, never git; `KASPULSE_REQUIRE_KEYS=1` fails closed if
   absent), and add ONE site block to the shared Caddyfile:
   `pulse.kascov.io { reverse_proxy 127.0.0.1:<port> }`, then reload Caddy.
   Caddy provisions the certificate on the first request.

> The VPS is Windows, so the `deploy/vps/*.sh` + systemd unit below are a
> Linux REFERENCE; the live box mirrors ironwood.live's Windows service +
> shared-Caddy setup. Exact Windows commands are finalized against the live
> box (must not disturb the running ironwood.live Caddy site).

`deploy/proxy/` and `deploy/hosting/` are an **ALTERNATIVE (Cloud Run +
kaspulse.web.app)** fallback, only if the oracle ever moves off the VPS —
not used by the active topology above.

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

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

**The service's working directory must be writable.** The oracle writes
`round.hwm` there (the `signer` bin writes `signer-round.hwm`) — the monotonic
round-slot high-water mark. `round` is derived from the wall clock, so a fresh
box with a correct clock is fine; the file exists only so a backwards clock step
can't re-issue a slot the committee already signed (that would be a genuine
equivocation proof against our own keys).

It is a **reservation, not a log**: the oracle writes `round + 150` *before*
issuing `round` (tmp-file + rename, so a crash mid-write can't leave a short
file), which means the number on disk is always an **upper bound** on everything
ever signed. A restart therefore skips up to ~60 s of round numbers. Gaps are
free; replays are not.

Operating rules, in the order they bite:

- **A VM snapshot restore rewinds the file together with the clock.** That is
  the dangerous case, not the safe one: the restored `round.hwm` is a floor from
  *before* the rounds that were published after the snapshot, and the restored
  clock agrees with it, so nothing detects the replay. After restoring a
  snapshot, **manually set `round.hwm` above the last round you published**
  (`curl -s https://pulse.kascov.io/v1/feed | jq .round` from the archive, or
  `date +%s%3N` divided by 400) *before* starting the service.
- Restoring an old `round.hwm` onto a box with a *correct forward* clock is
  harmless — the wall-clock slot is already far above the stale floor.
- Never run two oracles with the same keys in the same directory.
- A backwards clock step logs `KASPULSE_ROUND_REGRESSION` and keeps going at
  `hwm+1`. An unreadable file logs `KASPULSE_ROUND_HWM_UNREADABLE` and means
  there is **no** replay floor for that boot — treat it as a page.

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

- If a CDN is ever put in front, keep API clients pointed at the ORIGIN
  directly — today `pulse.kascov.io` straight to the VPS, not a cached edge
  (kascov lesson: CDN buffering vs `no-store` responses).
- **`KASPULSE_BIND=127.0.0.1` is load-bearing, not cosmetic.** The `/og` rate
  limiter only trusts `X-Forwarded-For` when the listener is bound to loopback
  (otherwise any client forges its own bucket), and it takes the **LAST** XFF
  entry because Caddy's bare `reverse_proxy 127.0.0.1:<port>` APPENDS the true
  peer. Change the hop count — put a CDN in front, or set an explicit
  `header_up X-Forwarded-For` in the Caddyfile — and that "last entry" rule has
  to be re-derived in `request_head()` (src/http.rs), or the whole internet goes
  back into one bucket.
- Built since this list was written: the `/v1/committee` pin artifact,
  expected-committee pinning in the SDK (`verify_with_committee`) and in the
  JS/Python clients (`verifyWithCommittee` / `verify_with_committee`), and
  per-IP `/og` rate limiting (120 req/min).
- Later, not built: committee-key **custody** (the five pubkeys are published,
  but they all live on one box — see README's honest status), pinning in the
  in-browser verifier `web/vendor/verify.js` (it still checks against the
  `signers` array in the same response, which the site copy states honestly),
  custom-domain mapping.
- Withdrawn, not later: hosted-committee signatures over `blake2b(price_bytes)`
  (`covenant.signatures`). It shipped, then came out on 2026-07-27 — the
  preimage binds no pair/expo/round/ts, so one feed's sigs spent any
  lower-strike gate on any pair. The bound `kaspulse/cov/v2` replacement is
  specified in docs/MESSAGE-FORMAT.md §8.0 and not shipped; the covenant guide
  uses a local demo committee, which is the correct pattern today.

# kaspulse verifying clients

Tiny, zero-dependency clients for the kaspulse oracle API — CORS-open, no keys.
Each one fetches a signed feed AND verifies it locally: 3-of-5 BIP340 Schnorr
signatures over blake2b-256 of `kaspulse/v2|PAIR|mant|expo|ts|round`, with the
signed fields checked against the JSON fields. Never trust the API — verify.

- **`js/kaspulse.mjs`** — Node 18+ / browser, native fetch + BigInt.
  `node js/kaspulse.mjs verify KAS/USD http://localhost:8080`
- **`py/kaspulse.py`** — Python 3.9+, stdlib only.
  `python3 py/kaspulse.py verify KAS/USD http://localhost:8080`
- **`../web/vendor/verify.js`** — the same crypto core, vendored for the site's
  in-browser verify button; `scripts/check-vendored.sh` fails the build on drift.

All three self-test at load (BIP340 official vector 0, a corrupted copy that
must fail, blake2b-256("abc")) and refuse to verify anything with a broken core.

Honest scope: they verify signatures, field binding, freshness and safety flags;
they do NOT re-fetch the exchanges and recompute the median — that is
`cargo run --bin verify`.

npm/PyPI publishing is a separate decision — each file is the whole client.

#!/usr/bin/env bash
# check-vendored.sh — guard against crypto-core drift.
#
# web/vendor/verify.js is a vendored copy of the canonical crypto core in
# clients/js/kaspulse.mjs (the region between the CRYPTO-CORE-BEGIN and
# CRYPTO-CORE-END markers). This script diffs the two regions byte-for-byte
# and exits 1 loudly if they have drifted. Run it after touching either file.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CANON="$ROOT/clients/js/kaspulse.mjs"
VENDOR="$ROOT/web/vendor/verify.js"

extract() { # print the core region of $1 (markers excluded)
  awk '/CRYPTO-CORE-BEGIN/{f=1;next} /CRYPTO-CORE-END/{f=0} f' "$1"
}

for f in "$CANON" "$VENDOR"; do
  if [ ! -f "$f" ]; then
    echo "check-vendored: MISSING $f" >&2
    exit 1
  fi
  if ! grep -q 'CRYPTO-CORE-BEGIN' "$f" || ! grep -q 'CRYPTO-CORE-END' "$f"; then
    echo "check-vendored: $f lacks CRYPTO-CORE-BEGIN/END markers" >&2
    exit 1
  fi
done

if ! diff -u <(extract "$CANON") <(extract "$VENDOR"); then
  cat >&2 <<'EOF'

check-vendored: DRIFT DETECTED between the crypto cores of
  clients/js/kaspulse.mjs   (canonical)
  web/vendor/verify.js      (vendored copy)
The in-browser verifier and the JS client MUST run identical crypto.
Edit clients/js/kaspulse.mjs, then copy the region between the
CRYPTO-CORE-BEGIN / CRYPTO-CORE-END markers into web/vendor/verify.js
verbatim (or regenerate the vendored file), and re-run this script.
EOF
  exit 1
fi

echo "check-vendored: OK — crypto core identical in clients/js/kaspulse.mjs and web/vendor/verify.js"

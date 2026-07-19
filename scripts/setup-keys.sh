#!/usr/bin/env bash
# One-time: pipe the 5 local committee key files into Secret Manager as
# `kaspulse-node-keys` (comma-separated 5×64-hex) WITHOUT ever echoing key
# material. Committee continuity IS the product — a keyless Cloud Run restart
# would silently mint a fresh committee and break every verifier and on-chain
# consumer that pinned the old pubkeys, which is why deploy.sh runs with
# KASPULSE_REQUIRE_KEYS=1 and this secret injected.
#
# Idempotent: creates the secret on first run, adds a version afterwards.
set -euo pipefail
cd "$(dirname "$0")/.."

PROJECT="${PROJECT:-$(gcloud config get-value project 2>/dev/null || true)}"
if [ -z "$PROJECT" ]; then
  echo "no GCP project — set PROJECT=<id> or 'gcloud config set project <id>'" >&2
  exit 1
fi
SECRET=kaspulse-node-keys
N=5

# validate every key file first (64 hex chars), without printing a byte of it
for i in $(seq 0 $((N - 1))); do
  f="kaspulse-node-$i.key"
  if [ ! -s "$f" ]; then
    echo "missing $f — run the oracle once locally to mint the committee, or restore the files from backup" >&2
    exit 1
  fi
  if ! tr -d ' \n' < "$f" | grep -qiE '^[0-9a-f]{64}$'; then
    echo "$f is not 64 hex chars — refusing to upload" >&2
    exit 1
  fi
done

# comma-join the raw key material straight into gcloud's stdin — no tmp files,
# no shell variables holding secrets, nothing echoed
payload() {
  for i in $(seq 0 $((N - 1))); do
    tr -d ' \n' < "kaspulse-node-$i.key"
    if [ "$i" -lt $((N - 1)) ]; then printf ','; fi
  done
}

gcloud services enable secretmanager.googleapis.com --project "$PROJECT" > /dev/null

if gcloud secrets describe "$SECRET" --project "$PROJECT" > /dev/null 2>&1; then
  payload | gcloud secrets versions add "$SECRET" --project "$PROJECT" --data-file=-
  echo "==> added a new version to secret $SECRET (project $PROJECT)"
else
  payload | gcloud secrets create "$SECRET" --project "$PROJECT" \
    --replication-policy=automatic --data-file=-
  echo "==> created secret $SECRET (project $PROJECT)"
fi

echo
echo "verify the deploy wiring (scripts/deploy.sh already passes this flag):"
echo "  --update-secrets KASPULSE_NODE_KEYS=$SECRET:latest"

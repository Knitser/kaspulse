#!/usr/bin/env bash
# deploy-hosting.sh — point https://kaspulse.web.app at the Cloud Run proxy.
# Run AFTER deploy-proxy.sh. Idempotent.
set -euo pipefail
cd "$(dirname "$0")"
firebase deploy --only hosting:kaspulse --project kascov-explorer
echo
echo "live: https://kaspulse.web.app"

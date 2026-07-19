#!/usr/bin/env bash
# deploy-proxy.sh — the tiny scale-to-zero Cloud Run proxy behind kaspulse.web.app.
#
#   ./deploy/proxy/deploy-proxy.sh <backend-sslip-host>
#   e.g. ./deploy/proxy/deploy-proxy.sh 203-0-113-7.sslip.io
#
# Costs pennies: min-instances 0, only wakes when someone visits. Idempotent —
# re-run with a new host if the VPS IP ever changes.
set -euo pipefail

BACKEND="${1:?usage: deploy-proxy.sh <backend-sslip-host>}"
PROJECT=kascov-explorer
REGION=europe-west4

gcloud run deploy kaspulse-proxy \
  --project "$PROJECT" --region "$REGION" \
  --source "$(dirname "$0")" \
  --allow-unauthenticated \
  --min-instances 0 --max-instances 1 \
  --memory 128Mi --cpu 1 \
  --update-env-vars "BACKEND_HOST=$BACKEND"

URL=$(gcloud run services describe kaspulse-proxy --project "$PROJECT" --region "$REGION" --format 'value(status.url)')
echo
echo "proxy live: $URL  (backend: https://$BACKEND)"
curl -fsS "$URL/health" && echo || echo "health via proxy not ready — is the VPS oracle warm?"

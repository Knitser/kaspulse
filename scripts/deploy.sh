#!/usr/bin/env bash
# Deploy the kaspulse oracle to Cloud Run — one stateful singleton service
# (live exchange WebSockets + in-memory signing state), europe-west4,
# min=max=1 instances. Idempotent: safe to re-run on every deploy.
#
# One-time prerequisites (manual, user-gated):
#   gcloud billing projects link <PROJECT> --billing-account=<ACCOUNT_ID>
#   scripts/setup-keys.sh          # committee keys -> Secret Manager
#   publish github.com/Knitser/kaspulse (DEPLOY.md §2 — the site links it
#   ~11 times and the zero-dep clients are only downloadable from it)
#
# Mirrors KasDev/scripts/deploy-worker.sh discipline: --update-env-vars only
# (never --set-env-vars), list-before-create monitoring, loud manual steps.
set -euo pipefail
cd "$(dirname "$0")/.."

PROJECT="${PROJECT:-$(gcloud config get-value project 2>/dev/null || true)}"
if [ -z "$PROJECT" ]; then
  echo "no GCP project — set PROJECT=<id> or 'gcloud config set project <id>'" >&2
  exit 1
fi
REGION=europe-west4
SERVICE=kaspulse
SECRET=kaspulse-node-keys

if [ "$(gcloud billing projects describe "$PROJECT" --format='value(billingEnabled)')" != "True" ]; then
  echo "billing is not enabled on $PROJECT — link a billing account first:" >&2
  echo "  gcloud billing projects link $PROJECT --billing-account=<ACCOUNT_ID>" >&2
  exit 1
fi

echo "==> enabling APIs"
gcloud services enable run.googleapis.com cloudbuild.googleapis.com \
  artifactregistry.googleapis.com monitoring.googleapis.com \
  secretmanager.googleapis.com --project "$PROJECT"

# DEPLOY BLOCKER: without the committee secret, the service would come up with
# KASPULSE_REQUIRE_KEYS=1, log KASPULSE_KEYS_MISSING and exit — by design.
if ! gcloud secrets describe "$SECRET" --project "$PROJECT" > /dev/null 2>&1; then
  echo "secret $SECRET does not exist — run scripts/setup-keys.sh first" >&2
  echo "(a keyless restart would mint a fresh committee and break every pinned verifier)" >&2
  exit 1
fi

echo "==> ensuring Artifact Registry repo 'kaspulse'"
gcloud artifacts repositories create kaspulse \
  --repository-format=docker --location=$REGION --project "$PROJECT" 2>/dev/null || true

IMAGE="$REGION-docker.pkg.dev/$PROJECT/kaspulse/kaspulse:latest"

echo "==> building $IMAGE via Cloud Build (Dockerfile builds --features og + fetches fonts)"
gcloud builds submit --tag "$IMAGE" --project "$PROJECT" .

# let the runtime service account read the committee secret (idempotent)
SA="$(gcloud projects describe "$PROJECT" --format='value(projectNumber)')-compute@developer.gserviceaccount.com"
gcloud secrets add-iam-policy-binding "$SECRET" --project "$PROJECT" \
  --member="serviceAccount:$SA" --role=roles/secretmanager.secretAccessor > /dev/null

# --update-env-vars MERGES with the previous revision's env — never use
# --set-env-vars here: it REPLACES the whole env and once silently disarmed a
# deploy key on kascov. BASE_URL: pass it if you have a domain; otherwise it
# is set to the *.run.app URL after the first deploy (below).
ENV_VARS="KASPULSE_REQUIRE_KEYS=1"
if [ -n "${BASE_URL:-}" ]; then ENV_VARS="$ENV_VARS,BASE_URL=$BASE_URL"; fi

echo "==> deploying $SERVICE to Cloud Run ($REGION)"
gcloud run deploy $SERVICE \
  --image "$IMAGE" \
  --project "$PROJECT" \
  --region $REGION \
  --allow-unauthenticated \
  --min-instances 1 \
  --max-instances 1 \
  --no-cpu-throttling \
  --cpu-boost \
  --cpu 1 \
  --memory 512Mi \
  --concurrency 250 \
  --timeout 300 \
  --port 8080 \
  --update-env-vars "$ENV_VARS" \
  --update-secrets "KASPULSE_NODE_KEYS=$SECRET:latest"

URL=$(gcloud run services describe $SERVICE --project "$PROJECT" --region $REGION --format='value(status.url)')

# no domain yet: absolute share/og/sitemap URLs come from the run.app origin
if [ -z "${BASE_URL:-}" ]; then
  echo "==> BASE_URL not provided — pointing it at the service URL ($URL)"
  gcloud run services update $SERVICE --project "$PROJECT" --region $REGION \
    --update-env-vars "BASE_URL=$URL" > /dev/null
fi

# ---- monitoring: uptime checks + log-token alerts, all list-before-create.
# `uptime create` is NOT idempotent (kascov accumulated 7 duplicate checks
# by Jul 18 before the look-before-create guard) — same discipline here.
HOST=$(echo "$URL" | sed 's|https://||')
ensure_uptime() {
  local name="$1" path="$2"
  if gcloud monitoring uptime list-configs --project "$PROJECT" \
      --filter="displayName='$name'" --format='value(name)' 2>/dev/null | grep -q .; then
    echo "    uptime check '$name' already exists"
    return 0
  fi
  gcloud monitoring uptime create "$name" \
    --resource-type=uptime-url \
    --resource-labels="host=$HOST,project_id=$PROJECT" \
    --path="$path" \
    --project "$PROJECT" 2>/dev/null \
    || echo "    (uptime create failed or CLI unsupported — create '$name' on $path in the console)"
}
echo "==> ensuring uptime checks (/health — NOT /healthz, GFE swallows it on run.app — and /v1/feed)"
ensure_uptime kaspulse-health /health
ensure_uptime kaspulse-feed /v1/feed

# log-based alert policy, idempotent by displayName ('policies create'
# happily makes duplicates, so look first)
ensure_log_alert() {
  local name="$1" log_regex="$2"
  if gcloud alpha monitoring policies list --project "$PROJECT" \
      --filter="displayName='$name'" --format='value(name)' 2>/dev/null | grep -q .; then
    echo "    alert policy '$name' already exists"
    return 0
  fi
  local tmp
  tmp=$(mktemp)
  # conditionMatchedLog requires alertStrategy.notificationRateLimit
  cat > "$tmp" <<EOF
{
  "displayName": "$name",
  "combiner": "OR",
  "enabled": true,
  "conditions": [
    {
      "displayName": "$name log match",
      "conditionMatchedLog": {
        "filter": "resource.type=\\"cloud_run_revision\\" AND resource.labels.service_name=\\"$SERVICE\\" AND textPayload=~\\"$log_regex\\""
      }
    }
  ],
  "alertStrategy": {
    "notificationRateLimit": { "period": "3600s" },
    "autoClose": "604800s"
  }
}
EOF
  gcloud alpha monitoring policies create --policy-from-file="$tmp" --project "$PROJECT" > /dev/null \
    && echo "    created alert policy '$name'" \
    || echo "    (could not create '$name' — console log filter: textPayload=~\"$log_regex\")"
  rm -f "$tmp"
}
echo "==> ensuring log-based alert policies (exact tokens the oracle logs)"
ensure_log_alert kaspulse-keys-missing "KASPULSE_KEYS_MISSING"
ensure_log_alert kaspulse-discovery-empty "KASPULSE_DISCOVERY_EMPTY"

echo ""
echo "    MANUAL STEP (user-gated): the uptime checks and alert policies have NO"
echo "    notification channel — nothing emails/pages you until one is attached."
echo "    Console > Monitoring > Alerting > pick policy > edit, attach your channel to:"
echo "      - kaspulse-keys-missing      (service refused to boot without the committee)"
echo "      - kaspulse-discovery-empty   (DEX auto-discovery returning near-empty sets)"
echo "      - kaspulse-health / kaspulse-feed uptime checks"
echo ""
echo "==> done. oracle: $URL"
echo "    /health · /v1/feed · /v1/feeds · /share/KAS-USD · /og/KAS-USD.png"
echo "    NOTE: if a CDN ever fronts this, keep API clients on the run.app origin"
echo "    directly (kascov lesson: CDN buffering + no-store don't mix)."

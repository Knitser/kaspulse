#!/usr/bin/env bash
# deploy-vps.sh — set up (or update) the kaspulse oracle on a Debian/Ubuntu VPS.
#
#   ./deploy/vps/deploy-vps.sh user@host
#
# Run FROM the repo root on the machine that holds the committee key files
# (kaspulse-node-{0..4}.key). Idempotent: re-run to pull + rebuild + restart.
#
# What it does, in order:
#   1. installs build deps + rustup + caddy on the VPS (first run only)
#   2. clones/pulls github.com/Knitser/kaspulse to /opt/kaspulse/app
#   3. cargo build --release --features og, fetches the OG-card fonts
#   4. copies the LOCAL committee key files to the VPS (committee continuity —
#      without them the oracle would mint a brand-new committee; chmod 600)
#   5. installs the systemd unit (loopback-only oracle) + Caddy site for
#      https://<ip-dashes>.sslip.io (free TLS, no domain needed)
#   6. health-checks the public URL and prints what to plug into the
#      Cloud Run proxy (deploy/proxy/deploy-proxy.sh)
set -euo pipefail

TARGET="${1:?usage: deploy-vps.sh user@host}"
APP_DIR=/opt/kaspulse/app
REPO=https://github.com/Knitser/kaspulse

say() { printf '\n\033[1;36m== %s\033[0m\n' "$*"; }

for k in kaspulse-node-0.key kaspulse-node-1.key kaspulse-node-2.key kaspulse-node-3.key kaspulse-node-4.key; do
  [ -f "$k" ] || { echo "missing $k — run from the repo root that holds the committee keys"; exit 1; }
done

say "1/6 base packages (first run may take a few minutes)"
ssh "$TARGET" 'set -e
  export DEBIAN_FRONTEND=noninteractive
  sudo apt-get update -qq
  sudo apt-get install -y -qq git curl build-essential pkg-config libssl-dev debian-keyring debian-archive-keyring apt-transport-https >/dev/null
  if ! command -v caddy >/dev/null; then
    curl -1sLf https://dl.cloudsmith.io/public/caddy/stable/gpg.key | sudo gpg --dearmor --yes -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
    curl -1sLf https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt | sudo tee /etc/apt/sources.list.d/caddy-stable.list >/dev/null
    sudo apt-get update -qq && sudo apt-get install -y -qq caddy >/dev/null
  fi
  if ! command -v cargo >/dev/null && [ ! -x "$HOME/.cargo/bin/cargo" ]; then
    curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y -q
  fi
  id -u kaspulse >/dev/null 2>&1 || sudo useradd --system --home /opt/kaspulse --shell /usr/sbin/nologin kaspulse
  sudo mkdir -p /opt/kaspulse && sudo chown "$USER" /opt/kaspulse'

say "2/6 clone/pull + build (release, og feature)"
ssh "$TARGET" "set -e
  export PATH=\"\$HOME/.cargo/bin:\$PATH\"
  if [ -d $APP_DIR/.git ]; then git -C $APP_DIR pull --ff-only; else git clone --depth 1 $REPO $APP_DIR; fi
  cd $APP_DIR && cargo build --release --features og
  ./scripts/fetch-fonts.sh"

say "3/6 committee keys (from this machine, never through git)"
scp -q kaspulse-node-{0..4}.key "$TARGET:/tmp/"
ssh "$TARGET" "set -e
  sudo mv /tmp/kaspulse-node-*.key $APP_DIR/
  sudo chown kaspulse:kaspulse $APP_DIR/kaspulse-node-*.key && sudo chmod 600 $APP_DIR/kaspulse-node-*.key
  sudo chown -R kaspulse:kaspulse /opt/kaspulse"

say "4/6 systemd unit"
IP=$(ssh "$TARGET" "curl -4fsS ifconfig.me")
SSLIP_HOST="$(echo "$IP" | tr . -).sslip.io"
sed "s|__APP_DIR__|$APP_DIR|g" deploy/vps/kaspulse.service | ssh "$TARGET" "sudo tee /etc/systemd/system/kaspulse.service >/dev/null"
ssh "$TARGET" "sudo systemctl daemon-reload && sudo systemctl enable --now kaspulse && sleep 2 && sudo systemctl is-active kaspulse"

say "5/6 caddy → https://$SSLIP_HOST"
sed "s|__SSLIP_HOST__|$SSLIP_HOST|g" deploy/vps/Caddyfile | ssh "$TARGET" "sudo tee /etc/caddy/Caddyfile >/dev/null"
ssh "$TARGET" "sudo systemctl reload caddy || sudo systemctl restart caddy"

say "6/6 health check (oracle needs ~40s of warmup on first boot)"
for i in $(seq 1 12); do
  if curl -fsS "https://$SSLIP_HOST/health" 2>/dev/null | grep -q '"ok":true'; then
    echo; echo "HEALTHY: https://$SSLIP_HOST/health"
    echo
    echo "Next: wire the public URL —"
    echo "  ./deploy/proxy/deploy-proxy.sh $SSLIP_HOST"
    echo "  ./deploy/hosting/deploy-hosting.sh"
    exit 0
  fi
  sleep 10
done
echo "not healthy yet — check: ssh $TARGET 'journalctl -u kaspulse -n 50'"
exit 1

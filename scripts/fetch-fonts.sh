#!/usr/bin/env bash
# Fetch the JetBrains Mono TTFs (+ their OFL license) that the OG-card
# renderer loads at RUNTIME from assets/fonts/. The repo never vendors font
# binaries — run this once locally, or let the Dockerfile run it at build
# time. Without the fonts the server still runs fine; /og/{PAIR}.png just
# returns 404 with a one-line note.
#
# Source: the official JetBrains Mono GitHub release, tag v2.304 (OFL-1.1).
set -euo pipefail
cd "$(dirname "$0")/.."

DIR=assets/fonts
TAG=v2.304
BASE="https://raw.githubusercontent.com/JetBrains/JetBrainsMono/$TAG"

mkdir -p "$DIR"
for f in JetBrainsMono-Regular.ttf JetBrainsMono-Bold.ttf; do
  if [ -s "$DIR/$f" ]; then
    echo "  $DIR/$f already present — skipping"
    continue
  fi
  echo "  fetching $f ($TAG)"
  curl -fsSL "$BASE/fonts/ttf/$f" -o "$DIR/$f.tmp"
  mv "$DIR/$f.tmp" "$DIR/$f"
done

# ship the license next to the fonts (OFL requires it to travel with them)
if [ ! -s "$DIR/OFL.txt" ]; then
  echo "  fetching OFL.txt"
  curl -fsSL "$BASE/OFL.txt" -o "$DIR/OFL.txt"
fi

echo "fonts ready in $DIR (JetBrains Mono $TAG, SIL OFL 1.1)"

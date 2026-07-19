JetBrains Mono Regular + Bold TTFs land here (plus their OFL.txt) via
scripts/fetch-fonts.sh — the OG-card renderer loads them at runtime and the
repo never vendors font binaries. Without them the server runs fine;
/og/{PAIR}.png returns 404 with a note.

#!/bin/sh
# Installs the latest petramux build: quits the app (its sessions keep running in the
# helper), swaps the app in /Applications, and opens it again.
set -e
app=/Applications/petramux.app
new="$(cd "$(dirname "$0")/.." && pwd)/src-tauri/target/release/bundle/macos/petramux.app"
[ -d "$new" ] || { echo "No build at $new. Run 'bun run tauri build' first." >&2; exit 1; }
running() { pgrep -xf "$app/Contents/MacOS/petramux" >/dev/null; }
if running; then
  osascript -e 'quit app "petramux"'
  for _ in $(seq 50); do running || break; sleep 0.2; done
  if running; then echo "petramux didn't quit; quit it and run this again." >&2; exit 1; fi
fi
ditto "$new" "$app"
open "$app"
echo "Installed the build from $(stat -f '%Sm' "$new/Contents/MacOS/petramux")."

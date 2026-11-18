#!/bin/sh
# Builds petramux for Apple silicon and Intel as one .dmg and publishes it as a GitHub release, for
# people who'd rather download it than build it. Usage: scripts/release.sh [--build-only]
# The version is the one in src-tauri/tauri.conf.json; bump it before each release.
#
# Each release also carries the update the app installs itself (petramux.app.tar.gz) and
# latest.json, which running copies check. Both are signed with the key in
# ~/.tauri/petramux-updater.key (its public half is in tauri.conf.json); lose that key and running
# copies can't take updates any more.
set -e
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
version=$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' src-tauri/tauri.conf.json | head -1)
key="$HOME/.tauri/petramux-updater.key"
[ -f "$key" ] || { echo "No update signing key at $key." >&2; exit 1; }
export TAURI_SIGNING_PRIVATE_KEY="$(cat "$key")" TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
# CI=true: the .dmg is made without scripting Finder to arrange its window
CI=true bun run tauri build --target universal-apple-darwin --bundles app,dmg \
  --config '{"bundle":{"createUpdaterArtifacts":true}}'
bundle=src-tauri/target/universal-apple-darwin/release/bundle
dmg=$(ls -t $bundle/dmg/*.dmg | head -1)
out="src-tauri/target/petramux-$version.dmg"
cp "$dmg" "$out"
tarball="src-tauri/target/petramux.app.tar.gz"
cp "$bundle/macos/petramux.app.tar.gz" "$tarball"
url="https://github.com/zjovy/petramux/releases/download/v$version/petramux.app.tar.gz"
sig=$(cat "$bundle/macos/petramux.app.tar.gz.sig")
cat > src-tauri/target/latest.json <<EOF
{
  "version": "$version",
  "pub_date": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "platforms": {
    "darwin-aarch64": { "signature": "$sig", "url": "$url" },
    "darwin-x86_64": { "signature": "$sig", "url": "$url" }
  }
}
EOF
echo "Built $out, $tarball and latest.json"
[ "$1" = "--build-only" ] && exit 0
gh release create "v$version" "$out" "$tarball" src-tauri/target/latest.json --title "petramux $version" --notes "Download petramux-$version.dmg, open it, and drag petramux to Applications. From 0.2.6 on, petramux updates itself: when a new version is ready, the sidebar shows Restart to update.

The first time you open it, macOS says it can't check the app (it isn't signed by an Apple developer account). Open System Settings → Privacy & Security and click Open Anyway, once.

Needs Claude Code 2.1.296 or later."

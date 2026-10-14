#!/bin/sh
# Builds petramux for Apple silicon and Intel as one .dmg and publishes it as a GitHub release, for
# people who'd rather download it than build it. Usage: scripts/release.sh [--build-only]
# The version is the one in src-tauri/tauri.conf.json; bump it before each release.
set -e
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
version=$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' src-tauri/tauri.conf.json | head -1)
rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
# CI=true: the .dmg is made without scripting Finder to arrange its window
CI=true bun run tauri build --target universal-apple-darwin --bundles dmg
dmg=$(ls -t src-tauri/target/universal-apple-darwin/release/bundle/dmg/*.dmg | head -1)
out="src-tauri/target/petramux-$version.dmg"
cp "$dmg" "$out"
echo "Built $out"
[ "$1" = "--build-only" ] && exit 0
gh release create "v$version" "$out" --title "petramux $version" --notes "Download petramux-$version.dmg, open it, and drag petramux to Applications.

The first time you open it, macOS says it can't check the app (it isn't signed by an Apple developer account). Open System Settings → Privacy & Security and click Open Anyway, once.

Needs Claude Code 2.1.296 or later."

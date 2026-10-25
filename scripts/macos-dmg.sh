#!/usr/bin/env bash
# Packs Farsight.app into a compressed disk image with the usual
# "drag Farsight to Applications" layout. macOS only (uses hdiutil).
# Usage: scripts/macos-dmg.sh <version> <path to Farsight.app> <out.dmg>
set -euo pipefail
version="$1" app="$2" out="$3"
staging="$(mktemp -d)"
trap 'rm -rf "$staging"' EXIT
cp -R "$app" "$staging/Farsight.app"
ln -s /Applications "$staging/Applications"
hdiutil create -volname "Farsight $version" -srcfolder "$staging" -ov -format UDZO "$out" >/dev/null
echo "Built $out"

#!/usr/bin/env bash
# Wraps the viewer in Farsight.app so macOS shows the Farsight logo in the
# Dock, Launchpad and Finder.
# Usage: scripts/macos-app.sh <version> <farsight binary> <out dir>
set -euo pipefail
version="$1" binary="$2" app="$3/Farsight.app"
root="$(cd "$(dirname "$0")/.." && pwd)"
install -d "$app/Contents/MacOS" "$app/Contents/Resources"
install -m 0755 "$binary" "$app/Contents/MacOS/farsight"
install -m 0644 "$root/dist/icons/Farsight.icns" "$app/Contents/Resources/Farsight.icns"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Farsight</string>
  <key>CFBundleDisplayName</key><string>Farsight</string>
  <key>CFBundleIdentifier</key><string>ca.akrotechnologies.Farsight</string>
  <key>CFBundleExecutable</key><string>farsight</string>
  <key>CFBundleIconFile</key><string>Farsight</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

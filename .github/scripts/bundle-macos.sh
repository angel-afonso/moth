#!/usr/bin/env bash
# Usage: bundle-macos.sh <binary> <version> <output dir>
#
# Builds <output dir>/Moth.app around a compiled binary: the executable, the icon, the
# license and an Info.plist with the version, so Finder, the Dock and Gatekeeper treat it as
# a regular app. Signing happens afterwards (see the Release workflow).
set -euo pipefail

binary="$1"
version="$2"
out="$3"
root="$(cd "$(dirname "$0")/../.." && pwd)"
app="$out/Moth.app"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/moth"
chmod +x "$app/Contents/MacOS/moth"
cp "$root/assets/branding/moth.icns" "$app/Contents/Resources/moth.icns"
cp "$root/LICENSE" "$app/Contents/Resources/LICENSE"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Moth</string>
  <key>CFBundleDisplayName</key><string>Moth</string>
  <key>CFBundleIdentifier</key><string>dev.moth.Moth</string>
  <key>CFBundleExecutable</key><string>moth</string>
  <key>CFBundleIconFile</key><string>moth</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" >/dev/null
echo "Built $app ($version)"

#!/usr/bin/env bash
# Build the desktop app as a macOS bundle with the Roostr icon:
# target/release/Roostr.app (open it, drag it to Applications or the Dock).
#
#   scripts/bundle-mac.sh              # build and bundle
#   scripts/bundle-mac.sh --install    # and move it to ~/Applications (the only copy)
#
# The icon is crates/roostr/assets/Roostr.icns: static/logo.png on Apple's
# icon grid (824px rounded square, 100px margin, on 1024).
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT=$(pwd)

cargo build --release -p roostr

VERSION=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "roostr"))')
APP="$ROOT/target/release/Roostr.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$ROOT/target/release/roostr" "$APP/Contents/MacOS/roostr"
cp "$ROOT/crates/roostr/assets/Roostr.icns" "$APP/Contents/Resources/Roostr.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key><string>Roostr</string>
	<key>CFBundleDisplayName</key><string>Roostr</string>
	<key>CFBundleIdentifier</key><string>space.roostr.desktop</string>
	<key>CFBundleExecutable</key><string>roostr</string>
	<key>CFBundleIconFile</key><string>Roostr</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>$VERSION</string>
	<key>CFBundleVersion</key><string>$VERSION</string>
	<key>LSMinimumSystemVersion</key><string>11.0</string>
	<key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
# Ad-hoc signed so Apple silicon runs it.
codesign --force --deep --sign - "$APP"
echo "Built $APP"

if [[ "${1:-}" == "--install" ]]; then
	mkdir -p "$HOME/Applications"
	rm -rf "$HOME/Applications/Roostr.app"
	# Moved, not copied: one Roostr.app on the Mac, so Spotlight and the Dock open this one.
	mv "$APP" "$HOME/Applications/Roostr.app"
	echo "Installed $HOME/Applications/Roostr.app"
fi

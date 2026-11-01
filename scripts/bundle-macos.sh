#!/bin/sh
# Builds target/release/notch-utils.app (ad-hoc signed) and target/release/notch-utils-macos.zip.
set -eu
cd "$(dirname "$0")/.."
cargo build --release
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
APP=target/release/notch-utils.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/notch-utils "$APP/Contents/MacOS/notch-utils"
cp assets/appicon/notch-utils.icns "$APP/Contents/Resources/notch-utils.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>notch-utils</string>
  <key>CFBundleDisplayName</key><string>notch-utils</string>
  <key>CFBundleIdentifier</key><string>io.github.fabianriegsinger.notch-utils</string>
  <key>CFBundleExecutable</key><string>notch-utils</string>
  <key>CFBundleIconFile</key><string>notch-utils</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSCameraUsageDescription</key><string>The Mirror tab shows your camera so you can check how you look.</string>
  <key>NSAppleEventsUsageDescription</key><string>Now Playing reads and controls the current track in Music and Spotify.</string>
</dict>
</plist>
PLIST
codesign --force --sign - --timestamp=none "$APP"
(cd target/release && rm -f notch-utils-macos.zip && ditto -c -k --keepParent notch-utils.app notch-utils-macos.zip)
echo "built $APP"

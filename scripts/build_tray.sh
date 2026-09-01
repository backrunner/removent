#!/usr/bin/env bash
# Build the RemoventTray release binary and package it as a minimal RemoventTray.app in dist/.
# Idempotent: re-running overwrites previous artifacts.
set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v swift >/dev/null 2>&1; then
    export DEVELOPER_DIR="${DEVELOPER_DIR:-/Applications/Xcode-beta.app/Contents/Developer}"
    SWIFT="$(xcrun -f swift)"
else
    SWIFT="$(command -v swift)"
fi

echo "==> swift build (release)"
"$SWIFT" build -c release --package-path tray

BIN="tray/.build/release/RemoventTray"
RES_BUNDLE="tray/.build/release/RemoventTray_RemoventTray.bundle"
APP="dist/RemoventTray.app"

echo "==> packaging $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/RemoventTray"

# SwiftPM resource bundle (Localizable.strings) — without it the tray UI loses localization.
if [ -d "$RES_BUNDLE" ]; then
    cp -R "$RES_BUNDLE" "$APP/Contents/Resources/"
else
    echo "WARNING: $RES_BUNDLE not found; tray UI strings will fall back to keys" >&2
fi

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleExecutable</key>
    <string>RemoventTray</string>
    <key>CFBundleIdentifier</key>
    <string>com.removent.tray</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleLocalizations</key>
    <array>
        <string>en</string>
        <string>zh-Hans</string>
    </array>
    <key>CFBundleName</key>
    <string>RemoventTray</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>LSUIElement</key>
    <true/>
</dict>
</plist>
PLIST

echo "==> done: $APP"

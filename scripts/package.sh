#!/usr/bin/env bash
# Package Removent.app (arm64) -> dist/Removent-<ver>-macos-arm64.zip
#
# Signing: when APPLE_SIGNING_IDENTITY is set (e.g. "Developer ID Application: Name (TEAMID)"),
# every binary is signed with the hardened runtime, inside-out. Otherwise the bundle is
# ad-hoc signed, which is only usable on the build machine (Gatekeeper rejects it elsewhere).
set -euo pipefail
cd "$(dirname "$0")/.."

source scripts/macos_env.sh
VERSION=$(python3 scripts/release_meta.py version)
BASE_VERSION=$(python3 scripts/release_meta.py base)
BUILD_VERSION=$(python3 scripts/release_meta.py build)
APP_NAME="Removent"
BUNDLE="dist/${APP_NAME}.app"
ZIP="dist/${APP_NAME}-${VERSION}-macos-arm64.zip"
IDENTITY="${APPLE_SIGNING_IDENTITY:-}"

echo "==> build release"
cargo build --locked --release -p removent-app -p removent-daemon

echo "==> build tray"
"$(dirname "$0")/build_tray.sh"

echo "==> bundle ${BUNDLE}"
rm -rf "$BUNDLE" "$ZIP"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources/zh-Hans.lproj"
cat > "$BUNDLE/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>CFBundleExecutable</key><string>removent-launcher</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
    <key>RemoventReleaseVersion</key><string>${VERSION}</string>
    <key>CFBundleIdentifier</key><string>io.removent.app</string>
    <key>CFBundleLocalizations</key>
    <array>
        <string>en</string>
        <string>zh-Hans</string>
    </array>
    <key>CFBundleName</key><string>${APP_NAME}</string>
    <key>CFBundleVersion</key><string>${BUILD_VERSION}</string>
    <key>CFBundleShortVersionString</key><string>${BASE_VERSION}</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSLocalNetworkUsageDescription</key>
    <string>Removent uses your local network to discover nearby Macs and connect to devices you choose.</string>
    <key>NSBonjourServices</key>
    <array>
        <string>_removent._udp</string>
    </array>
    <key>NSScreenCaptureUsageDescription</key>
    <string>Removent needs Screen Recording permission to share your screen with peers you approve.</string>
    <key>NSAccessibilityUsageDescription</key>
    <string>Removent needs Accessibility permission to inject keyboard and mouse input when this Mac is being controlled.</string>
</dict>
</plist>
PLIST

# Localized permission prompts (Simplified Chinese).
cat > "$BUNDLE/Contents/Resources/zh-Hans.lproj/InfoPlist.strings" <<'STRINGS'
"NSScreenCaptureUsageDescription" = "Removent 需要屏幕录制权限以向您批准的对方共享屏幕。";
"NSAccessibilityUsageDescription" = "Removent 需要辅助功能权限以在本机被控制时注入键鼠输入。";
"NSLocalNetworkUsageDescription" = "Removent 需要访问本地网络以发现附近的 Mac，并连接您选择的设备。";
STRINGS

cp target/release/removent "$BUNDLE/Contents/MacOS/removent"
xcrun swiftc -O -target arm64-apple-macosx13.0 scripts/launcher.swift \
    -o "$BUNDLE/Contents/MacOS/removent-launcher"
cp assets/branding/AppIcon.icns "$BUNDLE/Contents/Resources/AppIcon.icns"
# The daemon ships inside the bundle: the app locates removentd next to its own executable.
cp target/release/removentd "$BUNDLE/Contents/MacOS/removentd"

# Embed the tray: the main app auto-launches it on startup (main.rs autostart_tray looks in Contents/Helpers).
mkdir -p "$BUNDLE/Contents/Helpers"
cp -R "dist/RemoventTray.app" "$BUNDLE/Contents/Helpers/"

if [ -n "$IDENTITY" ]; then
    echo "==> Developer ID signing: $IDENTITY"
    # Inside-out: nested code first, outer bundle last. Hardened runtime + secure timestamp
    # are required for notarization.
    codesign --force --options runtime --timestamp --sign "$IDENTITY" \
        "$BUNDLE/Contents/Helpers/RemoventTray.app"
    codesign --force --options runtime --timestamp --sign "$IDENTITY" \
        "$BUNDLE/Contents/MacOS/removentd"
    codesign --force --options runtime --timestamp --sign "$IDENTITY" \
        "$BUNDLE/Contents/MacOS/removent"
    codesign --force --options runtime --timestamp --sign "$IDENTITY" "$BUNDLE"
    echo "==> verify signature"
    codesign --verify --deep --strict --verbose=2 "$BUNDLE"
else
    echo "==> WARNING: APPLE_SIGNING_IDENTITY not set; ad-hoc signing (local testing only)"
    codesign --force --deep -s - "$BUNDLE"
fi

echo "==> zip"
cd dist
ditto -c -k --keepParent "${APP_NAME}.app" "$(basename "$ZIP")"
cd ..

echo "==> artifact: $ZIP"
ls -la dist/

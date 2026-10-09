#!/usr/bin/env bash
# Package Removent.app (arm64) -> dist/Removent-<ver>-macos-arm64.zip
#
# Signing: when APPLE_SIGNING_IDENTITY is set (e.g. "Developer ID Application: Name (TEAMID)"),
# every binary is signed with the hardened runtime, inside-out. Otherwise the bundle is
# ad-hoc signed, which is only usable on the build machine (Gatekeeper rejects it elsewhere).
set -euo pipefail
# CI keeps signing keys under umask 077. Public app bundles must remain
# readable/executable by every account when installed in /Applications.
umask 022
cd "$(dirname "$0")/../.."

source scripts/build/macos_env.sh
SOURCE_SNAPSHOT=$(python3 scripts/build/build_stamp.py snapshot)
VERSION=$(python3 scripts/release/release_meta.py version)
BASE_VERSION=$(python3 scripts/release/release_meta.py base)
BUILD_VERSION=$(python3 scripts/release/release_meta.py build)
APP_NAME="Removent"
mkdir -p dist
BUILD_ROOT=$(mktemp -d "$PWD/dist/.removent-package.XXXXXX")
trap 'rm -rf "$BUILD_ROOT"' EXIT
BUNDLE="$BUILD_ROOT/${APP_NAME}.app"
ZIP="dist/${APP_NAME}-${VERSION}-macos-arm64.zip"
IDENTITY="${APPLE_SIGNING_IDENTITY:-}"
LOCAL_IDENTITY="${REMOVENT_LOCAL_SIGNING_IDENTITY:-}"
LOCAL_HOST_IDENTITY="${REMOVENT_LOCAL_HOST_SIGNING_IDENTITY:-$LOCAL_IDENTITY}"
if [ -n "$LOCAL_HOST_IDENTITY" ] && [ -z "$LOCAL_IDENTITY" ] && [ -z "$IDENTITY" ]; then
    echo 'REMOVENT_LOCAL_HOST_SIGNING_IDENTITY requires REMOVENT_LOCAL_SIGNING_IDENTITY' >&2
    exit 1
fi
CLOUD_ENVIRONMENT="${REMOVENT_CLOUDKIT_ENVIRONMENT:-Production}"
case "$CLOUD_ENVIRONMENT" in
    Development|Production) ;;
    *) echo "REMOVENT_CLOUDKIT_ENVIRONMENT must be Development or Production" >&2; exit 1 ;;
esac
if [ -n "$IDENTITY" ]; then
    : "${REMOVENT_CLOUDKIT_PROFILE:?set the provisioning profile for com.alkinum.removent.sync}"
    test -f "$REMOVENT_CLOUDKIT_PROFILE"
fi
if [ -n "${REMOVENT_CLOUDKIT_PROFILE:-}" ]; then
    test -f "$REMOVENT_CLOUDKIT_PROFILE"
    if [ -z "$IDENTITY$LOCAL_IDENTITY" ]; then
        echo 'CloudKit provisioning requires a signing identity' >&2
        exit 1
    fi
fi

echo "==> build release"
cargo build --locked --release -p removent-app -p removent-daemon -p removent-cli
TARGET_DIR=$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')

echo "==> build tray"
"$(dirname "$0")/build_tray.sh"
bash "$(dirname "$0")/build_cloud_sync.sh"

echo "==> bundle ${BUNDLE}"
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
    <key>CFBundleIdentifier</key><string>com.alkinum.removent</string>
    <key>CFBundleLocalizations</key>
    <array>
        <string>en</string>
        <string>zh-Hans</string>
    </array>
    <key>CFBundleName</key><string>${APP_NAME}</string>
    <key>CFBundleVersion</key><string>${BUILD_VERSION}</string>
    <key>CFBundleShortVersionString</key><string>${BASE_VERSION}</string>
    <key>LSMinimumSystemVersion</key><string>26.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSLocalNetworkUsageDescription</key>
    <string>Removent uses your local network to discover remote desktop services and connect to devices you choose.</string>
    <key>NSBonjourServices</key>
    <array>
        <string>_removent._udp</string>
        <string>_rfb._tcp</string>
        <string>_rdp._tcp</string>
        <string>_ms-wbt-server._tcp</string>
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
"NSLocalNetworkUsageDescription" = "Removent 需要访问本地网络以发现远程桌面服务，并连接您选择的设备。";
STRINGS

cp "$TARGET_DIR/release/removent" "$BUNDLE/Contents/MacOS/removent"
xcrun swiftc -O -target arm64-apple-macosx26.0 apps/installer/main.swift \
    -o "$BUNDLE/Contents/MacOS/removent-launcher"
cp assets/branding/AppIcon.icns "$BUNDLE/Contents/Resources/AppIcon.icns"
# TCC needs a stable app identity for the actual process performing capture.
python3 scripts/build/package_host.py "$BUNDLE" "$TARGET_DIR/release/removentd" "$BASE_VERSION" "$BUILD_VERSION"
cp "$TARGET_DIR/release/removent-cli" "$BUNDLE/Contents/MacOS/removent-cli"

# Embed the tray: the main app auto-launches it on startup (main.rs autostart_tray looks in Contents/Helpers).
mkdir -p "$BUNDLE/Contents/Helpers"
cp -R "dist/RemoventTray.app" "$BUNDLE/Contents/Helpers/"
cp -R "dist/RemoventSync.app" "$BUNDLE/Contents/Helpers/"
python3 scripts/build/build_stamp.py write --app "$BUNDLE" --expected "$SOURCE_SNAPSHOT"

if [ -n "$IDENTITY" ]; then
    echo "==> Signing: $IDENTITY ($CLOUD_ENVIRONMENT CloudKit)"
    python3 scripts/build/configure_cloud_sync.py "$BUNDLE/Contents/Helpers/RemoventSync.app" --profile "$REMOVENT_CLOUDKIT_PROFILE" --environment "$CLOUD_ENVIRONMENT"
    codesign --force --options runtime --timestamp --sign "$IDENTITY" \
        --entitlements "$BUNDLE/Contents/Helpers/removent-sync.entitlements" \
        "$BUNDLE/Contents/Helpers/RemoventSync.app"
    rm "$BUNDLE/Contents/Helpers/removent-sync.entitlements"
    python3 scripts/build/configure_cloud_sync.py "$BUNDLE/Contents/Helpers/RemoventSync.app" --verify --environment "$CLOUD_ENVIRONMENT"
    # Inside-out: nested code first, outer bundle last. Hardened runtime + secure timestamp
    # are required for notarization.
    codesign --force --options runtime --timestamp --sign "$IDENTITY" \
        "$BUNDLE/Contents/Helpers/RemoventTray.app"
    codesign --force --options runtime --timestamp --sign "$IDENTITY" \
        "$BUNDLE/Contents/Helpers/RemoventHost.app"
    codesign --force --options runtime --timestamp --sign "$IDENTITY" \
        --identifier com.alkinum.removent.cli "$BUNDLE/Contents/MacOS/removent-cli"
    codesign --force --options runtime --timestamp --sign "$IDENTITY" \
        --identifier com.alkinum.removent.desktop "$BUNDLE/Contents/MacOS/removent"
    codesign --force --options runtime --timestamp --sign "$IDENTITY" "$BUNDLE"
    echo "==> verify signature"
    codesign --verify --deep --strict --verbose=2 "$BUNDLE"
elif [ -n "$LOCAL_IDENTITY" ]; then
    echo "==> Local signing with a stable identity: $LOCAL_IDENTITY"
    if [ -n "${REMOVENT_CLOUDKIT_PROFILE:-}" ]; then
        python3 scripts/build/configure_cloud_sync.py "$BUNDLE/Contents/Helpers/RemoventSync.app" --profile "$REMOVENT_CLOUDKIT_PROFILE" --environment "$CLOUD_ENVIRONMENT"
        codesign --force --options runtime --timestamp=none --sign "$LOCAL_IDENTITY" \
            --entitlements "$BUNDLE/Contents/Helpers/removent-sync.entitlements" \
            "$BUNDLE/Contents/Helpers/RemoventSync.app"
        rm "$BUNDLE/Contents/Helpers/removent-sync.entitlements"
        python3 scripts/build/configure_cloud_sync.py "$BUNDLE/Contents/Helpers/RemoventSync.app" --verify --environment "$CLOUD_ENVIRONMENT"
    else
        codesign --force --timestamp=none --sign "$LOCAL_IDENTITY" "$BUNDLE/Contents/Helpers/RemoventSync.app"
    fi
    codesign --force --timestamp=none --sign "$LOCAL_IDENTITY" "$BUNDLE/Contents/Helpers/RemoventTray.app"
    codesign --force --timestamp=none --sign "$LOCAL_HOST_IDENTITY" "$BUNDLE/Contents/Helpers/RemoventHost.app"
    codesign --force --timestamp=none --sign "$LOCAL_IDENTITY" --identifier com.alkinum.removent.cli "$BUNDLE/Contents/MacOS/removent-cli"
    codesign --force --timestamp=none --sign "$LOCAL_IDENTITY" --identifier com.alkinum.removent.desktop "$BUNDLE/Contents/MacOS/removent"
    codesign --force --timestamp=none --sign "$LOCAL_IDENTITY" "$BUNDLE"
    codesign --verify --deep --strict --verbose=2 "$BUNDLE"
else
    echo "==> WARNING: APPLE_SIGNING_IDENTITY not set; ad-hoc signing (local testing only)"
    codesign --force --deep -s - "$BUNDLE"
fi

# Publish only the fully built and signed bundle. A failed build must leave
# the previously runnable dist app intact.
rm -rf "dist/${APP_NAME}.app"
mv "$BUNDLE" "dist/${APP_NAME}.app"
echo "==> zip"
cd dist
ditto -c -k --keepParent "${APP_NAME}.app" "$(basename "$ZIP")"
cd ..

echo "==> artifact: $ZIP"
ls -la dist/

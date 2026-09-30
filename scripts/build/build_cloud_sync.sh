#!/usr/bin/env bash
# Build a real app bundle so the CloudKit caller can carry its own profile.
set -euo pipefail
umask 022
cd "$(dirname "$0")/../.."
source scripts/build/macos_env.sh
APP="dist/RemoventSync.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
xcrun swiftc -O -swift-version 5 -target arm64-apple-macosx26.0 \
    packages/apple-cloud-sync/Sources/*.swift apps/cloud-sync-helper/SyncHelper/main.swift \
    -o "$APP/Contents/MacOS/RemoventSync"
python3 - "$APP" <<'PY'
import pathlib, plistlib, sys
sys.path.insert(0, 'scripts/release')
from release_meta import BASE, BUILD
path = pathlib.Path(sys.argv[1]) / 'Contents/Info.plist'
path.write_bytes(plistlib.dumps({
    'CFBundleIdentifier': 'com.alkinum.removent.sync',
    'CFBundleExecutable': 'RemoventSync', 'CFBundleName': 'Removent Sync',
    'CFBundlePackageType': 'APPL', 'CFBundleShortVersionString': BASE,
    'CFBundleVersion': str(BUILD), 'LSMinimumSystemVersion': '26.0', 'LSUIElement': True,
}))
PY

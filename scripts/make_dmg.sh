#!/usr/bin/env bash
# Create dist/Removent-<ver>-macos-arm64.dmg from dist/Removent.app.
# Uses plain hdiutil (no external dependencies): app + /Applications symlink in a UDZO image.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
APP_NAME="Removent"
BUNDLE="dist/${APP_NAME}.app"
DMG="dist/${APP_NAME}-${VERSION}-macos-arm64.dmg"
STAGING="$(mktemp -d /tmp/removent-dmg.XXXXXX)"
trap 'rm -rf "$STAGING"' EXIT

[ -d "$BUNDLE" ] || { echo "error: $BUNDLE missing; run scripts/package.sh first" >&2; exit 1; }

cp -R "$BUNDLE" "$STAGING/"
ln -s /Applications "$STAGING/Applications"

rm -f "$DMG"
hdiutil create -volname "$APP_NAME" -srcfolder "$STAGING" -ov -format UDZO "$DMG"

echo "==> artifact: $DMG"

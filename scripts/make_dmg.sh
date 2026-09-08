#!/usr/bin/env bash
# Assemble a Retina Finder installer without Finder automation (works in CI).
set -euo pipefail
umask 022
cd "$(dirname "$0")/.."
VERSION=$(python3 scripts/release_meta.py version)
DMG="dist/Removent-${VERSION}-macos-arm64.dmg"
WORK=$(mktemp -d /tmp/removent-dmg.XXXXXX)
MOUNT="$WORK/mount"
MOUNTED=false
cleanup() {
    if [ "$MOUNTED" = true ]; then
        if ! hdiutil detach "$MOUNT" -quiet; then
            echo "warning: could not detach $MOUNT; temporary image retained at $WORK" >&2
            return
        fi
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT
[ -d dist/Removent.app ] || { echo 'error: build dist/Removent.app first' >&2; exit 1; }
mkdir -p "$WORK/stage/.background" "$MOUNT"
ditto dist/Removent.app "$WORK/stage/Removent.app"
ln -s /Applications "$WORK/stage/Applications"
cp assets/branding/dmg-background.tiff "$WORK/stage/.background/background.tiff"
hdiutil create -quiet -fs HFS+ -volname Removent -srcfolder "$WORK/stage" -format UDRW "$WORK/installer.dmg"
hdiutil attach -quiet -nobrowse -noautoopen -mountpoint "$MOUNT" "$WORK/installer.dmg"
MOUNTED=true
python3 scripts/dmg_layout.py "$MOUNT"
python3 scripts/verify_dmg_layout.py "$MOUNT"
# Finder reads the root .DS_Store; bless --openfolder is unsupported on Apple Silicon.
hdiutil detach -quiet "$MOUNT"
MOUNTED=false
hdiutil convert "$WORK/installer.dmg" -quiet -format UDZO -imagekey zlib-level=9 -o "$DMG" -ov
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
    codesign --force --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$DMG"
fi
hdiutil verify "$DMG"
echo "Created $DMG"

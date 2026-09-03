#!/usr/bin/env bash
# Full local release: build + Developer ID sign + notarize + DMG + update manifest.
#
# Required environment:
#   APPLE_SIGNING_IDENTITY  e.g. "Developer ID Application: Your Name (TEAMID)"
#   plus notarization credentials — see scripts/notarize.sh.
#
# Optional environment:
#   UPDATE_SIGNING_KEY_FILE  Ed25519 PEM for signing latest.json
#                            (default ~/.config/removent/update-signing-key.pem)
#   GITHUB_REPO_URL          release URL base (default https://github.com/backrunner/removent)
#
# Produces in dist/:
#   Removent-<ver>-macos-arm64.zip   (notarized app archive)
#   Removent-<ver>-macos-arm64.dmg   (notarized + stapled installer)
#   latest.json                      (auto-update manifest)
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
APP_NAME="Removent"
BUNDLE="dist/${APP_NAME}.app"
ZIP="dist/${APP_NAME}-${VERSION}-macos-arm64.zip"
DMG="dist/${APP_NAME}-${VERSION}-macos-arm64.dmg"

: "${APPLE_SIGNING_IDENTITY:?set APPLE_SIGNING_IDENTITY to your Developer ID Application identity}"

scripts/package.sh

# Notarize the zipped app first so the .app can be stapled before it goes into the DMG.
scripts/notarize.sh "$ZIP"
xcrun stapler staple "$BUNDLE"
xcrun stapler validate "$BUNDLE"

scripts/make_dmg.sh
scripts/notarize.sh "$DMG"

echo "==> generate latest.json"
REPO_URL="${GITHUB_REPO_URL:-https://github.com/backrunner/removent}"
# The updater downloads the zip and swaps the .app, so the manifest points at it.
MIN_PROTO=$(sed -n 's/.*PROTO_VERSION: u16 = \([0-9][0-9]*\).*/\1/p' crates/proto/src/constants.rs | head -1)
: "${MIN_PROTO:?could not read PROTO_VERSION from crates/proto/src/constants.rs}"
# Signing key: env override, else the maintainer's local key. Without a key the
# manifest is left unsigned (gen_latest.py warns; development builds only).
KEY_FILE="${UPDATE_SIGNING_KEY_FILE:-$HOME/.config/removent/update-signing-key.pem}"
KEY_ARGS=()
if [ -f "$KEY_FILE" ]; then
    KEY_ARGS=(--key "$KEY_FILE")
else
    echo "==> WARNING: no update-signing key at $KEY_FILE; latest.json will be unsigned" >&2
fi
python3 scripts/gen_latest.py "$VERSION" \
    "$REPO_URL/releases/download/v${VERSION}/$(basename "$ZIP")" \
    --file "$ZIP" --min-proto "$MIN_PROTO" "${KEY_ARGS[@]}" > dist/latest.json

echo "==> release artifacts:"
ls -la dist/

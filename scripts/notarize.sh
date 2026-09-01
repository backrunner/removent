#!/usr/bin/env bash
# Notarize (and, where the format supports it, staple) a build artifact via xcrun notarytool.
#
# Usage: scripts/notarize.sh <file.dmg|file.zip|App.app>
#
# Credentials — one of two sets, via environment:
#   App Store Connect API key (preferred):
#     APPLE_API_KEY_PATH  path to AuthKey_<KEY_ID>.p8
#     APPLE_API_KEY_ID    the key id
#     APPLE_API_ISSUER    the issuer UUID
#   Apple ID (fallback):
#     APPLE_ID            Apple account email
#     APPLE_PASSWORD      app-specific password
#     APPLE_TEAM_ID       10-char team id
set -euo pipefail

FILE="${1:?usage: $0 <file.dmg|file.zip|App.app>}"

if [ -n "${APPLE_API_KEY_PATH:-}" ]; then
    CREDENTIALS=(--key "$APPLE_API_KEY_PATH" --key-id "${APPLE_API_KEY_ID:?set APPLE_API_KEY_ID}" --issuer "${APPLE_API_ISSUER:?set APPLE_API_ISSUER}")
elif [ -n "${APPLE_ID:-}" ]; then
    CREDENTIALS=(--apple-id "$APPLE_ID" --password "${APPLE_PASSWORD:?set APPLE_PASSWORD}" --team-id "${APPLE_TEAM_ID:?set APPLE_TEAM_ID}")
else
    echo "error: set APPLE_API_KEY_PATH/APPLE_API_KEY_ID/APPLE_API_ISSUER or APPLE_ID/APPLE_PASSWORD/APPLE_TEAM_ID" >&2
    exit 1
fi

echo "==> notarytool submit: $FILE"
xcrun notarytool submit "$FILE" "${CREDENTIALS[@]}" --wait --timeout 30m

case "$FILE" in
    *.dmg)
        xcrun stapler staple "$FILE"
        codesign --verify --verbose=4 "$FILE"
        spctl --assess --type open --context context:primary-signature --verbose=4 "$FILE"
        xcrun stapler validate "$FILE"
        ;;
    *.app)
        xcrun stapler staple "$FILE"
        spctl --assess --type execute --verbose=4 "$FILE"
        xcrun stapler validate "$FILE"
        ;;
    *.zip)
        # Zip archives carry no staple; the notarization ticket covers their contents.
        ;;
esac

echo "==> notarized: $FILE"

#!/usr/bin/env bash
# Notarize (and, where the format supports it, staple) a build artifact via xcrun notarytool.
#
# Usage: scripts/notarize.sh <file.dmg|file.zip|App.app>
#
# Credentials via NOTARY_KEYCHAIN_PROFILE or one of two sets, via environment:
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

if [ -n "${NOTARY_KEYCHAIN_PROFILE:-}" ]; then
    CREDENTIALS=(--keychain-profile "$NOTARY_KEYCHAIN_PROFILE")
elif [ -n "${APPLE_API_KEY_PATH:-}" ]; then
    CREDENTIALS=(--key "$APPLE_API_KEY_PATH" --key-id "${APPLE_API_KEY_ID:?set APPLE_API_KEY_ID}" --issuer "${APPLE_API_ISSUER:?set APPLE_API_ISSUER}")
elif [ -n "${APPLE_ID:-}" ]; then
    : "${APPLE_PASSWORD:?set APPLE_PASSWORD}"
    CREDENTIALS=(--apple-id "$APPLE_ID" --team-id "${APPLE_TEAM_ID:?set APPLE_TEAM_ID}")
else
    echo "error: set APPLE_API_KEY_PATH/APPLE_API_KEY_ID/APPLE_API_ISSUER or APPLE_ID/APPLE_PASSWORD/APPLE_TEAM_ID" >&2
    exit 1
fi

echo "==> notarytool submit: $FILE"
RESULT=$(mktemp /tmp/removent-notary.XXXXXX)
trap 'rm -f "$RESULT"' EXIT
if [ "${CREDENTIALS[0]}" = --apple-id ]; then
    # Feed the secure password prompt through stdin, never process arguments.
    printf '%s\n' "$APPLE_PASSWORD" | xcrun notarytool submit "$FILE" "${CREDENTIALS[@]}" --wait --timeout 30m --output-format json > "$RESULT"
else
    xcrun notarytool submit "$FILE" "${CREDENTIALS[@]}" --wait --timeout 30m --output-format json > "$RESULT"
fi
python3 - "$RESULT" <<'PYTHON'
import json, sys
r = json.load(open(sys.argv[1]))
print('Notarization:', r.get('status'), 'submission:', r.get('id'))
if r.get('status') != 'Accepted':
    raise SystemExit('Notarization was not accepted; inspect notarytool log before publishing')
PYTHON

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

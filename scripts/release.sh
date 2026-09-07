#!/usr/bin/env bash
# Build → sign → notarize → staple → archive → sign manifest. No unsigned releases.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/macos_env.sh
VERSION=$(python3 scripts/release_meta.py version)
ZIP="dist/Removent-${VERSION}-macos-arm64.zip"
DMG="dist/Removent-${VERSION}-macos-arm64.dmg"
: "${APPLE_SIGNING_IDENTITY:?set the Developer ID Application signing identity}"
export UPDATE_SIGNING_KEY_FILE="${UPDATE_SIGNING_KEY_FILE:-$HOME/.config/removent/update-signing-key.pem}"
[ -f "$UPDATE_SIGNING_KEY_FILE" ] || { echo 'error: missing update signing key' >&2; exit 1; }
if [ -z "${NOTARY_KEYCHAIN_PROFILE:-}${APPLE_API_KEY_PATH:-}${APPLE_ID:-}" ]; then
    echo 'error: configure NOTARY_KEYCHAIN_PROFILE or notarization credentials' >&2; exit 1
fi
# Reject a mismatched key before spending time building or signing artifacts.
python3 scripts/gen_latest.py --check-key "$UPDATE_SIGNING_KEY_FILE"
scripts/package.sh
scripts/notarize.sh "$ZIP"
xcrun stapler staple dist/Removent.app
xcrun stapler validate dist/Removent.app
spctl --assess --type execute --verbose=4 dist/Removent.app
# The update archive must contain the stapled app too, not the pre-notary copy.
rm "$ZIP"
ditto -c -k --keepParent dist/Removent.app "$ZIP"
scripts/make_dmg.sh
scripts/notarize.sh "$DMG"
REPO_URL="${GITHUB_REPO_URL:-https://github.com/backrunner/removent}"
MIN_PROTO=$(sed -n 's/.*PROTO_VERSION: u16 = \([0-9][0-9]*\).*/\1/p' crates/proto/src/constants.rs | head -1)
python3 scripts/gen_latest.py "$VERSION" \
    "$REPO_URL/releases/download/v${VERSION}/$(basename "$ZIP")" \
    --file "$ZIP" --min-proto "$MIN_PROTO" --key "$UPDATE_SIGNING_KEY_FILE" \
    --notes-file "docs/releases/v${VERSION}.md" > dist/latest.json
(cd dist && shasum -a 256 "$(basename "$ZIP")" "$(basename "$DMG")" latest.json > SHA256SUMS)
python3 scripts/verify_release.py
echo "Release v${VERSION} verified and ready to publish."

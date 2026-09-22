#!/usr/bin/env bash
# Publish only the exact, verified tagged source. Use immutable version tags.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/macos_env.sh
VERSION=$(python3 scripts/release_meta.py version)
TAG="v${VERSION}"
test -z "$(git status --porcelain)" || { echo 'error: commit the release source before publishing' >&2; exit 1; }
test "$(git rev-parse "$TAG^{commit}")" = "$(git rev-parse HEAD)"
REMOTE_SHA=$(git ls-remote --tags origin "refs/tags/$TAG^{}" "refs/tags/$TAG" | awk 'END {print $1}')
test "$REMOTE_SHA" = "$(git rev-parse HEAD)" || { echo 'error: push the matching tag before publishing' >&2; exit 1; }
python3 scripts/verify_release.py
python3 scripts/package_relay.py --version "$TAG" --verify
RELAY_ASSETS=()
for RELAY_PLATFORM in linux-x86_64 linux-aarch64 macos-universal; do
    RELAY_ASSET="dist/relay/removent-relay-${TAG}-${RELAY_PLATFORM}.tar.gz"
    RELAY_ASSETS+=("$RELAY_ASSET" "$RELAY_ASSET.sha256")
done
gh release create "$TAG" --repo backrunner/removent \
    "dist/Removent-${VERSION}-macos-arm64.dmg" \
    "dist/Removent-${VERSION}-macos-arm64.zip" \
    dist/latest.json dist/SHA256SUMS \
    "${RELAY_ASSETS[@]}" \
    --verify-tag --title "Removent $TAG" \
    --notes-file "docs/releases/${TAG}.md" --latest

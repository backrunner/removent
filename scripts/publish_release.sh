#!/usr/bin/env bash
# Publish only the exact, verified tagged source. No mutable beta/latest tags.
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
ARGS=()
if [ "$(python3 scripts/release_meta.py channel)" = beta ]; then
    ARGS+=(--prerelease --latest=false)
else
    ARGS+=(--latest)
fi
gh release create "$TAG" --repo backrunner/removent \
    "dist/Removent-${VERSION}-macos-arm64.dmg" \
    "dist/Removent-${VERSION}-macos-arm64.zip" \
    dist/latest.json dist/SHA256SUMS \
    --verify-tag --title "Removent $TAG" \
    --notes-file "docs/releases/${TAG}.md" "${ARGS[@]}"

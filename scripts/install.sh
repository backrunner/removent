#!/usr/bin/env bash
# Removent quick installer — downloads the latest release DMG and installs Removent.app
# into /Applications.
#
#   curl -fsSL https://raw.githubusercontent.com/backrunner/removent/main/scripts/install.sh | bash
#
# Environment overrides:
#   REMOVENT_REPO   GitHub "owner/repo" (default: backrunner/removent)
#   REMOVENT_VERSION  install a specific tag (e.g. v0.1.0) instead of the latest release
set -euo pipefail

REPO="${REMOVENT_REPO:-backrunner/removent}"
APP_NAME="Removent"

if [ "$(uname -s)" != "Darwin" ]; then
    echo "error: Removent currently supports macOS only." >&2
    exit 1
fi
if [ "$(uname -m)" != "arm64" ]; then
    echo "error: Removent currently ships Apple Silicon (arm64) builds only." >&2
    exit 1
fi

TMP="$(mktemp -d /tmp/removent-install.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

if [ -n "${REMOVENT_VERSION:-}" ]; then
    API_URL="https://api.github.com/repos/${REPO}/releases/tags/${REMOVENT_VERSION}"
else
    API_URL="https://api.github.com/repos/${REPO}/releases/latest"
fi

echo "==> resolving latest release from ${REPO}"
RELEASE_JSON="$(curl -fsSL "$API_URL")"
DMG_URL="$(printf '%s' "$RELEASE_JSON" | grep '"browser_download_url"' | grep '\.dmg"' | head -1 | sed 's/.*"\(https[^"]*\)".*/\1/')"
VERSION="$(printf '%s' "$RELEASE_JSON" | grep -m1 '"tag_name"' | sed 's/.*: *"\([^"]*\)".*/\1/')"

if [ -z "$DMG_URL" ]; then
    echo "error: no .dmg asset found in release ${VERSION:-<unknown>} of ${REPO}." >&2
    exit 1
fi

echo "==> downloading ${APP_NAME} ${VERSION}"
curl -fSL --progress-bar "$DMG_URL" -o "$TMP/removent.dmg"

echo "==> mounting disk image"
MOUNT="$(hdiutil attach -nobrowse -readonly -mountpoint "$TMP/mnt" "$TMP/removent.dmg" >/dev/null && echo "$TMP/mnt")"

echo "==> installing to /Applications"
# Remove any previous install first: cp -R into an existing .app merges contents,
# which can leave stale binaries (e.g. an old removentd) behind on upgrades.
if [ -d "/Applications/${APP_NAME}.app" ]; then
    rm -rf "/Applications/${APP_NAME}.app" 2>/dev/null || sudo rm -rf "/Applications/${APP_NAME}.app"
fi
if ! cp -R "$MOUNT/${APP_NAME}.app" /Applications/ 2>/dev/null; then
    echo "    /Applications is not writable; asking for sudo"
    sudo cp -R "$MOUNT/${APP_NAME}.app" /Applications/
fi
hdiutil detach "$MOUNT" >/dev/null

cat <<EOF
==> ${APP_NAME} ${VERSION} installed to /Applications/${APP_NAME}.app

Next steps:
  1. Open ${APP_NAME} from Launchpad or:  open -a ${APP_NAME}
  2. On first launch, grant Screen Recording and Accessibility permissions
     when prompted (required to share this Mac's screen and accept input).
EOF

#!/usr/bin/env bash
# Source from macOS build scripts. Avoid changing the machine's xcode-select.
if ! xcrun -f metal >/dev/null 2>&1; then
    for REMOVENT_XCODE in /Applications/Xcode.app /Applications/Xcode-beta.app; do
        if [ -d "$REMOVENT_XCODE" ]; then
            export DEVELOPER_DIR="$REMOVENT_XCODE/Contents/Developer"
            break
        fi
    done
fi
export MACOSX_DEPLOYMENT_TARGET=13.0
if [ "$(uname -m)" != arm64 ]; then
    echo 'error: this release supports Apple Silicon; use an arm64 runner' >&2
    exit 1
fi

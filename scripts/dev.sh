#!/usr/bin/env bash
# Incremental build + supervised development session (macOS).
set -euo pipefail
cd "$(dirname "$0")/.."

build=1
tray=1
build_only=0
for arg in "$@"; do
    case "$arg" in
        --no-build) build=0 ;;
        --no-tray) tray=0 ;;
        --build-only) build_only=1 ;;
        -h|--help)
            echo "Usage: scripts/dev.sh [--no-build] [--no-tray] [--build-only]"
            echo "Build and start app, daemon and tray. Ctrl-C stops this session."
            echo "REMOVENT_DATA_DIR overrides the default userdata/dev directory."
            exit 0 ;;
        *) echo "Unknown option: $arg (see --help)" >&2; exit 2 ;;
    esac
done

if [[ "$(uname -s)" != Darwin ]]; then
    echo "Removent development requires macOS." >&2
    exit 1
fi
if [[ "$build" == 1 ]]; then
    # GPUI needs Metal from full Xcode; do not change the system xcode-select.
    if ! xcrun -f metal >/dev/null 2>&1; then
        for xcode in /Applications/Xcode.app /Applications/Xcode-beta.app; do
            if [[ -d "$xcode" ]]; then
                export DEVELOPER_DIR="$xcode/Contents/Developer"
                break
            fi
        done
    fi
    if ! xcrun -f metal >/dev/null 2>&1; then
        echo "Metal compiler not found. Install full Xcode and its Metal toolchain." >&2
        exit 1
    fi
    cargo build --locked -p removent-app -p removent-daemon
    if [[ "$tray" == 1 ]]; then
        swift build -c debug --package-path tray
    fi
fi

# cargo metadata respects CARGO_TARGET_DIR and .cargo/config.toml.
target_dir="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
app="$target_dir/debug/removent"
daemon="$target_dir/debug/removentd"
tray_bin="$PWD/tray/.build/debug/RemoventTray"
for bin in "$app" "$daemon"; do
    [[ -x "$bin" ]] || { echo "Missing $bin. Run without --no-build." >&2; exit 1; }
done
if [[ "$tray" == 1 && ! -x "$tray_bin" ]]; then
    echo "Missing $tray_bin. Run without --no-build." >&2
    exit 1
fi
[[ "$build_only" == 1 ]] && exit 0

export REMOVENT_DATA_DIR="${REMOVENT_DATA_DIR:-$PWD/userdata/dev}"
mkdir -p "$REMOVENT_DATA_DIR"
export REMOVENT_DATA_DIR="$(cd "$REMOVENT_DATA_DIR" && pwd)"
# Supervise the exact debug helpers here. Release autostart must not select
# an older Swift release build, a packaged tray, or a different data directory.
export REMOVENT_NO_TRAY=1
export REMOVENT_NO_UPDATE_CHECK="${REMOVENT_NO_UPDATE_CHECK:-1}"
export REMOVENT_DEV_APP="$app"
export RUST_LOG="${RUST_LOG:-info}"
pids=()
cleanup() {
    trap - EXIT INT TERM
    # macOS Bash 3.2 treats an empty array as unset under nounset.
    if [[ -n "${pids[*]:-}" ]]; then
        for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done
        for pid in "${pids[@]}"; do wait "$pid" 2>/dev/null || true; done
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# Do not take over an existing development daemon/session.
if [[ -S "$REMOVENT_DATA_DIR/run/removentd.sock" ]]; then
    if python3 - "$REMOVENT_DATA_DIR/run/removentd.sock" <<'PY'
import socket, sys
try:
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(0.5)
        client.connect(sys.argv[1])
except OSError:
    sys.exit(1)
PY
    then
        echo "A daemon is already using $REMOVENT_DATA_DIR. Stop that session or choose another REMOVENT_DATA_DIR." >&2
        exit 1
    fi
fi

echo "Development data: $REMOVENT_DATA_DIR"
echo "Ctrl-C stops the app, daemon and tray started by this script."
"$daemon" &
pids+=("$!")
if [[ "$tray" == 1 ]]; then
    "$tray_bin" &
    pids+=("$!")
fi
"$app" &
pids+=("$!")
wait "${pids[${#pids[@]}-1]}"

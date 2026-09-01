#!/usr/bin/env bash
# tray integration test: start a fake daemon (Python UDS server), run the tray
# debug binary, verify the log output, then clean up all processes.
# REMOVENT_TRAY_NO_ALERTS=1 keeps the tray from showing alerts.
set -uo pipefail

cd "$(dirname "$0")/../.."

TMPDIR_TEST="$(mktemp -d /tmp/removent-tray-test.XXXXXX)"
TRAY_LOG="$TMPDIR_TEST/tray.log"
DAEMON_LOG="$TMPDIR_TEST/daemon.log"
TRAY_PID=""
DAEMON_PID=""

cleanup() {
    [ -n "$TRAY_PID" ] && kill "$TRAY_PID" 2>/dev/null
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null
    rm -rf "$TMPDIR_TEST"
}
trap cleanup EXIT

if ! command -v swift >/dev/null 2>&1; then
    export DEVELOPER_DIR="${DEVELOPER_DIR:-/Applications/Xcode-beta.app/Contents/Developer}"
    SWIFT="$(xcrun -f swift)"
else
    SWIFT="$(command -v swift)"
fi

echo "==> swift build (debug)"
"$SWIFT" build --package-path tray || { echo "FAIL: build failed"; exit 1; }

echo "==> starting fake daemon (data dir ${TMPDIR_TEST})"
python3 tray/Tests/fake_daemon.py "$TMPDIR_TEST" >"$DAEMON_LOG" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 20); do
    [ -S "$TMPDIR_TEST/run/removentd.sock" ] && break
    sleep 0.2
done
[ -S "$TMPDIR_TEST/run/removentd.sock" ] || { echo "FAIL: fake daemon not ready"; cat "$DAEMON_LOG"; exit 1; }

echo "==> starting tray binary"
REMOVENT_DATA_DIR="$TMPDIR_TEST" REMOVENT_TRAY_NO_ALERTS=1 \
    tray/.build/debug/RemoventTray >"$TRAY_LOG" 2>&1 &
TRAY_PID=$!

sleep 6

kill "$TRAY_PID" 2>/dev/null
wait "$TRAY_PID" 2>/dev/null
TRAY_PID=""

echo "==> tray log:"
cat "$TRAY_LOG"

FAIL=0
check() {
    if grep -q "$1" "$TRAY_LOG"; then
        echo "PASS: $2"
    else
        echo "FAIL: $2 (missing log line: $1)"
        FAIL=1
    fi
}

check "connected to daemon" "connected to UDS socket"
check "status: running, port 7999" "status response fields parsed correctly"
check "sessions 1" "session list parsed correctly"
check "pairing PIN received: 482913" "pairing_pin event handled"
check "admission request received: 客厅 iPad" "admission_request event handled"
check "admission request #42 resolved: denied" "admission_resolved event handled"

echo "==> fake daemon log:"
cat "$DAEMON_LOG"
grep -q '"type": "status"\|"type":"status"' "$DAEMON_LOG" \
    && echo "PASS: daemon received status poll" \
    || { echo "FAIL: daemon received no status poll"; FAIL=1; }

if [ "$FAIL" -eq 0 ]; then
    echo "==> integration test passed"
else
    echo "==> integration test has failures"
fi
exit "$FAIL"

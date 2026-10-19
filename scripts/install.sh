#!/usr/bin/env bash
# Release installer for a local terminal or SSH session. Requires only macOS tools.
# curl -fsSL https://raw.githubusercontent.com/backrunner/removent/main/scripts/install.sh | bash
set -euo pipefail
umask 077

fail() { printf 'removent: %s\n' "$*" >&2; exit 1; }
usage() {
    cat <<'EOF'
Usage: install.sh [--version vX.Y.Z] [--app-dir DIR] [--start | --login | --no-login] [--verify-only]
  --version TAG   Pin a stable or beta release (default: latest stable).
  --app-dir DIR   Absolute application directory (default: /Applications).
  Default         Start the service and enable login startup unless explicitly disabled.
  --start         Start now, preserving the current login preference.
  --login         Explicitly enable startup at future user logins.
  --no-login      Install without starting; disable login startup.
  --verify-only   Download and verify the release without installing it.
Run as the target Mac user, without sudo. Only application file operations elevate.
Existing data, sharing settings and explicit login preferences are preserved.
A running service is refreshed; an explicitly stopped service stays stopped unless requested.
Quit Desktop and the menu bar app and finish remote sessions before upgrading.
First-time Screen Recording and Accessibility approval requires the Mac's GUI.
Environment: REMOVENT_VERSION, REMOVENT_REPO (default: backrunner/removent).
EOF
}
version=${REMOVENT_VERSION:-}
repo=${REMOVENT_REPO:-backrunner/removent}
app_dir=/Applications
start=auto
login=auto
verify_only=no
while [ "$#" -gt 0 ]; do
    case "$1" in
        --version) [ "$#" -ge 2 ] || fail '--version needs a tag'; version=$2; shift 2 ;;
        --app-dir) [ "$#" -ge 2 ] || fail '--app-dir needs a path'; app_dir=$2; shift 2 ;;
        --start) start=yes; [ "$login" != auto ] || login=preserve; shift ;;
        --login) login=yes; start=yes; shift ;;
        --no-login) login=no; start=no; shift ;;
        --verify-only) verify_only=yes; shift ;;
        --help|-h) usage; exit 0 ;;
        *) fail "Unknown option: $1 (see --help)" ;;
    esac
done
[ "$verify_only" = no ] || [ "$start" != yes ] || fail '--verify-only cannot start a service'
case "$app_dir" in /*) ;; *) fail '--app-dir must be absolute' ;; esac
case "$app_dir" in *$'\n'*|*$'\r'*) fail 'Application path must not contain newlines' ;; esac
[[ "$repo" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || fail 'Invalid REMOVENT_REPO'
[ "$(uname -s)/$(uname -m)" = Darwin/arm64 ] || fail 'Removent requires an Apple silicon Mac (arm64)'
major=$(sw_vers -productVersion | awk -F. '{print $1}')
[[ "$major" =~ ^[0-9]+$ ]] && [ "$major" -ge 26 ] || fail 'macOS 26 or newer is required'
[ "$(id -u)" != 0 ] || fail 'Run as the target Mac user without sudo; file installation elevates separately'
[ -n "${HOME:-}" ] || fail 'HOME is required'
for cmd in curl diskutil shasum ditto codesign spctl plutil ps launchctl mktemp; do
    command -v "$cmd" >/dev/null 2>&1 || fail "Missing macOS tool: $cmd"
done
# Older supported macOS versions may not provide the diskutil image subcommand.
# Probe without attaching anything; never retry a failed attach with another tool.
image_tool=diskutil
if ! diskutil image attach --help >/dev/null 2>&1; then
    command -v hdiutil >/dev/null 2>&1 || fail 'Missing macOS tool: hdiutil'
    image_tool=hdiutil
fi
attach_image() {
    if [ "$image_tool" = diskutil ]; then
        diskutil image attach --mountOptions nobrowse --readOnly --mountPoint "$1" "$2"
    else
        hdiutil attach -nobrowse -readonly -mountpoint "$1" "$2"
    fi
}
eject_image() {
    if [ "$image_tool" = diskutil ]; then diskutil eject "$1"
    else hdiutil detach "$1"; fi
}
release="https://github.com/$repo/releases"
download() {
    curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
        --tlsv1.2 --retry 3 --connect-timeout 15 "$@"
}
if [ -z "$version" ]; then
    latest=$(download --output /dev/null --write-out '%{url_effective}' "$release/latest")
    case "$latest" in "$release"/tag/*) version=${latest##*/} ;; *) fail 'Cannot resolve the latest stable release' ;; esac
fi
[[ "$version" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-beta\.[1-9][0-9]*)?$ ]] || fail 'Use a tag such as v0.1.4 or v0.1.4-beta.2'
asset="Removent-${version#v}-macos-arm64.dmg"
work=$(mktemp -d "${TMPDIR:-/tmp}/removent-install.XXXXXXXX")
mount="$work/mnt"
mounted=no
elevate=no
transaction=
lock=
backed_up=no
swap_started=no
installed=no
committed=no
service_touched=no
new_start_attempted=no
resume=no
cli=
old_cli=
stopped=false
login_before=false
as_admin() { if [ "$elevate" = yes ]; then sudo -- "$@"; else "$@"; fi; }
cleanup() {
    local result=$?
    trap - EXIT
    set +e
    if [ "$committed" = no ]; then
        # Signals can arrive after mv succeeds but before its flag assignment.
        if [ "$swap_started" = yes ]; then
            [ ! -d "$transaction/previous.app" ] || backed_up=yes
            if [ ! -d "$transaction/new.app" ] && [ -d "$destination" ]; then installed=yes; fi
        fi
        # Keep the old bundle until the replacement verifies and its old service resumes.
        local restore=yes
        if [ "$installed" = yes ]; then
            if [ "$new_start_attempted" = yes ] && ! "$cli" daemon stop >/dev/null 2>&1; then
                restore=no
            elif ! as_admin mv -- "$destination" "$transaction/failed.app"; then
                restore=no
            fi
        fi
        if [ "$backed_up" = yes ] && [ "$restore" = yes ]; then
            if ! as_admin mv -- "$transaction/previous.app" "$destination"; then
                printf 'removent: Restore the previous app from %s/previous.app\n' "$transaction" >&2
                transaction=
                result=1
            fi
        fi
        if [ "$restore" = no ]; then
            printf 'removent: Rollback needs manual recovery; app backups retained at %s\n' "$transaction" >&2
            transaction=
            result=1
        fi
        if [ "$service_touched" = yes ] && [ "$resume" = yes ] && [ "$restore" = yes ]; then
            if ! "$old_cli" daemon start; then
                printf 'removent: Previous service could not resume; run "%s" daemon start\n' "$old_cli" >&2
                result=1
            fi
        fi
    fi
    [ -z "$transaction" ] || as_admin rm -rf -- "$transaction" || result=1
    [ -z "$lock" ] || as_admin rmdir -- "$lock" || result=1
    if [ "$mounted" = yes ]; then
        if ! eject_image "$mount" >/dev/null; then
            printf 'removent: Disk image remains mounted at %s; eject it manually.\n' "$mount" >&2
            exit 1
        fi
    fi
    rm -rf -- "$work"
    exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
printf '==> Downloading %s\n' "$asset"
download --output "$work/$asset" "$release/download/$version/$asset"
download --output "$work/SHA256SUMS" "$release/download/$version/SHA256SUMS"
expected=$(awk -v name="$asset" '$2 == name {if (NF != 2 || length($1) != 64 || $1 ~ /[^a-f0-9]/) exit 1; value=$1; count++} END {if (count != 1) exit 1; print value}' "$work/SHA256SUMS") || fail 'Missing or invalid release SHA-256 checksum'
actual=$(shasum -a 256 "$work/$asset" | awk '{print $1}')
[ "$actual" = "$expected" ] || fail 'SHA-256 mismatch; existing installation was not changed'
attach_image "$mount" "$work/$asset" >/dev/null
mounted=yes
source_app="$mount/Removent.app"
verify_app() {
    local bundle=$1 component identifier
    [ -d "$bundle" ] && [ ! -L "$bundle" ] || fail 'Release does not contain a regular Removent.app'
    [ "$(plutil -extract CFBundleIdentifier raw -o - "$bundle/Contents/Info.plist")" = com.alkinum.removent ] || fail 'Unexpected application identifier'
    [ "$(plutil -extract RemoventReleaseVersion raw -o - "$bundle/Contents/Info.plist")" = "${version#v}" ] || fail 'Application version does not match the requested release'
    while read -r component identifier; do
        codesign --verify --all-architectures --strict -R "=anchor apple generic and identifier \"$identifier\" and certificate leaf[subject.OU] = \"PB8H83VL3Z\" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists" "$bundle/$component"
    done <<'EOF'
. com.alkinum.removent
Contents/MacOS/removent com.alkinum.removent.desktop
Contents/MacOS/removent-cli com.alkinum.removent.cli
Contents/Helpers/RemoventHost.app com.alkinum.removent.host
Contents/Helpers/RemoventTray.app com.alkinum.removent.tray
Contents/Helpers/RemoventSync.app com.alkinum.removent.sync
EOF
    codesign --verify --deep --strict "$bundle"
    spctl --assess --type execute "$bundle"
    [ -x "$bundle/Contents/MacOS/removent-cli" ] && [ -x "$bundle/Contents/MacOS/removentd" ] || fail 'Release CLI or daemon is missing'
}
verify_app "$source_app"
if [ "$verify_only" = yes ]; then
    printf '==> Verified %s: SHA-256, all component signatures, version and Gatekeeper assessment.\n' "$version"
    exit 0
fi
[ ! -L "$app_dir" ] || fail 'Application directory must not be a symlink'
if [ ! -d "$app_dir" ]; then
    mkdir -p -- "$app_dir" 2>/dev/null || elevate=yes
elif [ ! -w "$app_dir" ]; then elevate=yes
fi
if [ "$elevate" = yes ]; then
    command -v sudo >/dev/null 2>&1 || fail "Use --app-dir \"$HOME/Applications\" or install sudo"
    if ! sudo -n true 2>/dev/null; then
        # stdin may be the script itself. Prompt only through the terminal.
        if [ -r /dev/tty ] && ( : </dev/tty ) 2>/dev/null; then sudo -v
        else fail "Directory requires sudo; use ssh -t, pre-authorize sudo, or --app-dir \"$HOME/Applications\""; fi
    fi
fi
as_admin mkdir -p -- "$app_dir"
app_dir=$(cd "$app_dir" && pwd -P)
destination="$app_dir/Removent.app"
old_cli="$destination/Contents/MacOS/removent-cli"
cli=$old_cli
[ ! -L "$destination" ] || fail 'Refusing to replace a symlink destination'
[ ! -e "$destination" ] || [ -d "$destination" ] || fail 'Existing destination must be an application directory'
if ! as_admin mkdir -- "$app_dir/.removent-install.lock" 2>/dev/null; then
    fail "Another installer is active, or a stale lock exists at $app_dir/.removent-install.lock; inspect it before removing"
fi
lock="$app_dir/.removent-install.lock"
transaction=$(as_admin mktemp -d "$app_dir/.removent-install.XXXXXXXX")
# sudo-created staging directories are mode 0700; signature checks run as the user.
if [ "$elevate" = yes ]; then as_admin chown "$(id -u)" "$transaction"; fi
as_admin ditto "$source_app" "$transaction/new.app"
verify_app "$transaction/new.app"
# Match actual executable paths rather than killing by process name. Any user or
# custom-data daemon still running from this bundle blocks the final swap.
processes() { ps -wwaxo command= | awk -v prefix="$destination/Contents/" 'index($0, prefix) == 1 {print}'; }
if [ -e "$destination" ]; then
    [ -x "$old_cli" ] || fail 'Existing CLI is missing; stop all existing components before a manual repair'
    active=$(processes | awk '!/\/removentd( |$)|\/RemoventHost( |$)/')
    [ -z "$active" ] || fail 'Quit the installed Desktop and menu bar app before upgrading'
    "$old_cli" daemon service-status > "$work/service.json"
    boolean() {
        local value
        value=$(plutil -extract "$1" raw -o - "$work/service.json")
        case "$value" in true|false) printf '%s' "$value" ;; *) fail "Invalid service status: $1" ;; esac
    }
    managed=$(boolean managed)
    reachable=$(boolean reachable)
    stopped=$(boolean stopped_by_user)
    login_before=$(boolean launch_at_login)
    if [ "$managed" = true ] || [ "$reachable" = true ]; then
        if [ "$reachable" = true ]; then
            "$old_cli" daemon status > "$work/daemon.json"
            [ "$(plutil -extract sessions json -o - "$work/daemon.json")" = '[]' ] || fail 'Finish active remote sessions before upgrading'
        fi
        [ "$stopped" = true ] || resume=yes
        if [ "$resume" = yes ]; then
            launchctl print "gui/$(id -u)" >/dev/null 2>&1 || fail 'Service refresh requires this SSH user to have a logged-in graphical session'
        fi
        service_touched=yes
        "$old_cli" daemon stop > /dev/null
    fi
fi
if [ "$start" = auto ]; then
    if [ "$stopped" = true ]; then start=no; else start=yes; fi
fi
if [ "$start" = yes ]; then
    launchctl print "gui/$(id -u)" >/dev/null 2>&1 || fail 'Starting the service requires this SSH user to have a logged-in graphical session; use --no-login to install only'
fi
[ -z "$(processes)" ] || fail 'A process still uses the installed bundle (possibly another user/data directory); stop it before upgrading'
swap_started=yes
if [ -e "$destination" ]; then
    as_admin mv -- "$destination" "$transaction/previous.app"
    backed_up=yes
fi
as_admin mv -- "$transaction/new.app" "$destination"
installed=yes
verify_app "$destination"
if [ "$resume" = yes ] || [ "$start" = yes ]; then
    new_start_attempted=yes
    "$cli" daemon start
fi
if [ "$login" = yes ]; then
    "$cli" daemon login-on
elif [ "$login" = no ]; then
    "$cli" daemon login-off
elif [ "$login" = preserve ]; then
    # Record an explicit start-only choice for the installed bundle's first launch.
    if [ "$login_before" = true ]; then "$cli" daemon login-on
    else "$cli" daemon login-off; fi
else
    # The downloaded release may predate the idempotent installation-default API.
    daemon_help=$("$cli" daemon --help)
    case "$daemon_help" in
        *install-defaults*) "$cli" daemon install-defaults ;;
        *) [ "$stopped" = true ] || "$cli" daemon login-on ;;
    esac
fi
committed=yes
printf '==> Installed Removent %s at %s\n' "$version" "$destination"
cli_setup_failed=no
cli_setup="$destination/Contents/Resources/install-cli.sh"
if [ -f "$cli_setup" ]; then
    bash "$cli_setup" "$destination" || cli_setup_failed=yes
else
    printf '==> This release predates automatic terminal commands; use the full CLI path below.\n'
fi
if [ "$resume" = yes ] || [ "$start" = yes ]; then
    "$cli" daemon service-status
    "$cli" daemon status
fi
cat <<EOF
CLI: "$cli" daemon service-status
To start: "$cli" daemon start
To enable sharing: "$cli" daemon enable
To start at future logins: "$cli" daemon login-on
Screen sharing requires Screen Recording and Accessibility approval in the Mac's
System Settings and a logged-in graphical session. Check actual grants with:
  "$cli" daemon status
After setup, use "$cli" pairing generate to authorize a connection by code.
EOF
[ "$cli_setup_failed" = no ] || fail 'App installed, but terminal command setup needs attention; see the error above'

#!/bin/sh
# Bootstrap only. All subsequent configuration and service control is native.
set -eu
umask 077

fail() { printf 'removent-relay: %s\n' "$*" >&2; exit 1; }
usage() {
    echo 'Usage: install_relay.sh [--version vX.Y.Z] [--bin-dir DIR] [--service-dir DIR] [--no-setup] [-- SETUP_OPTIONS]'
    echo 'Linux: downloads a static binary, then runs its systemd setup wizard.'
    echo 'macOS 13+ (Apple Silicon/Intel): installs a user launchd relay service.'
    echo '--no-setup installs only the CLI on either platform.'
}
version=${REMOVENT_RELAY_VERSION:-}
bin_dir=/usr/local/bin
setup=yes
service_dir=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --version) [ "$#" -ge 2 ] || fail '--version needs a tag'; version=$2; shift 2 ;;
        --bin-dir) [ "$#" -ge 2 ] || fail '--bin-dir needs a path'; bin_dir=$2; shift 2 ;;
        --service-dir) [ "$#" -ge 2 ] || fail '--service-dir needs a path'; service_dir=$2; shift 2 ;;
        --no-setup) setup=no; shift ;;
        --help|-h) usage; exit 0 ;;
        --) shift; break ;;
        *) fail "Unknown option: $1" ;;
    esac
done
case "$bin_dir" in /*) ;; *) fail '--bin-dir must be absolute' ;; esac
case "$service_dir" in ''|/*) ;; *) fail '--service-dir must be absolute' ;; esac
[ ! -L "$bin_dir" ] || fail 'Binary directory must not be a symlink'
for cmd in curl tar awk mktemp install; do command -v "$cmd" >/dev/null 2>&1 || fail "Install $cmd first"; done
case "$(uname -s)/$(uname -m)" in
    Linux/x86_64) platform=linux-x86_64 ;;
    Linux/aarch64|Linux/arm64) platform=linux-aarch64 ;;
    Darwin/arm64|Darwin/x86_64)
        platform=macos-universal
        major=$(sw_vers -productVersion | awk -F. '{print $1}')
        [ "$major" -ge 13 ] || fail 'macOS 13 or newer is required'
        ;;
    *) fail 'Supported: Linux x86_64/ARM64 and macOS Apple Silicon/Intel' ;;
esac
if [ "$setup" = no ] && [ "$#" -gt 0 ]; then fail 'Setup options cannot be used when setup is disabled'; fi
configured=no
case "$platform" in
    macos-*)
        if [ "$setup" = yes ]; then
            [ "$(id -u)" != 0 ] || fail 'Run the macOS installer as your login user, without sudo; it elevates binary installation only'
            [ -n "${HOME:-}" ] || fail 'HOME is required for macOS service setup'
        fi
        if [ -n "$service_dir" ]; then
            if [ -e "$service_dir/server.toml" ]; then configured=yes; fi
        elif [ -e "${HOME:-}/Library/Application Support/removent-relay/server.toml" ] || [ -e "${HOME:-}/Library/LaunchAgents/com.alkinum.removent.relay.plist" ]; then configured=yes
        fi
        ;;
    *)
        [ -z "$service_dir" ] || fail '--service-dir is only available on macOS'
        if [ -e /etc/removent-relay/server.toml ] || [ -e /etc/systemd/system/removent-relay.service ]; then configured=yes; fi
        ;;
esac
if [ "$setup" = yes ] && [ "$configured" = yes ]; then
    [ "$#" -eq 0 ] || fail 'Already configured; edit server.toml and use check/restart instead of setup options'
    setup=no
    echo 'Existing relay configuration detected; service and automatic startup states will be preserved.'
fi
if [ "$setup" = yes ]; then
    case "$platform" in
        linux-*)
            [ "$bin_dir" = /usr/local/bin ] || fail 'Service setup requires /usr/local/bin; use --no-setup for a custom directory'
            [ -d /run/systemd/system ] || fail 'Service setup requires systemd (247+); use --no-setup for a standalone binary'
            ;;
    esac
fi
release=https://github.com/backrunner/removent/releases
download() { curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --tlsv1.2 --retry 3 --connect-timeout 15 "$@"; }
if [ -z "$version" ]; then
    latest=$(download --output /dev/null --write-out '%{url_effective}' "$release/latest")
    case "$latest" in "$release"/tag/*) version=${latest##*/} ;; *) fail 'Cannot resolve the latest stable release' ;; esac
fi
printf '%s\n' "$version" | awk '/^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-beta\.[1-9][0-9]*)?$/ {ok=1} END {exit !ok}' || fail 'Use a release tag such as v0.1.3 or v0.1.3-beta.1'
asset="removent-relay-${version}-${platform}.tar.gz"
work=$(mktemp -d "${TMPDIR:-/tmp}/removent-relay.XXXXXXXX")
pending=
as_root() {
    if [ "$elevate" = yes ]; then sudo -- "$@"; else "$@"; fi
}
cleanup() { rm -rf "$work"; if [ -n "$pending" ]; then as_root rm -f -- "$pending"; fi; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
echo "Downloading $asset"
download --output "$work/$asset" "$release/download/$version/$asset"
download --output "$work/SHA256SUMS" "$release/download/$version/$asset.sha256"
expected=$(awk -v name="$asset" '$2 == name && length($1) == 64 && $1 !~ /[^a-f0-9]/ {value=$1; count++} END {if (count != 1 || NR != 1) exit 1; print value}' "$work/SHA256SUMS") || fail 'Invalid release checksum file'
if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$work/$asset" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$work/$asset" | awk '{print $1}')
else
    fail 'Install sha256sum or shasum first'
fi
[ "$actual" = "$expected" ] || fail 'SHA-256 mismatch; existing installation was not changed'
[ "$(tar -tzf "$work/$asset")" = removent-relay ] || fail 'Unexpected archive contents'
tar -tvzf "$work/$asset" | awk 'substr($0,1,1) != "-" {exit 1}' || fail 'Archive must contain a regular binary'
tar -xOzf "$work/$asset" removent-relay > "$work/removent-relay"
chmod 700 "$work/removent-relay"
[ "$("$work/removent-relay" --version)" = "removent-relay ${version#v}" ] || fail 'Binary version does not match the release'

elevate=no
if [ "$(id -u)" != 0 ] && { [ "$bin_dir" = /usr/local/bin ] || [ ! -w "$bin_dir" ]; }; then
    command -v sudo >/dev/null 2>&1 || fail 'Run as root, or use --no-setup --bin-dir with a writable directory'
    elevate=yes
fi
as_root mkdir -p -- "$bin_dir"
[ ! -L "$bin_dir/removent-relay" ] || fail 'Refusing to replace a symlink'
[ ! -e "$bin_dir/removent-relay" ] || [ -f "$bin_dir/removent-relay" ] || fail 'Existing installation must be a regular file'
pending=$(as_root mktemp "$bin_dir/.removent-relay.XXXXXXXX")
as_root install -m 755 "$work/removent-relay" "$pending"
as_root mv -f -- "$pending" "$bin_dir/removent-relay"
pending=
echo "Installed $bin_dir/removent-relay ($version)"
setup_relay() {
    case "$platform" in
        macos-*)
            if [ -n "$service_dir" ]; then "$bin_dir/removent-relay" --service-dir "$service_dir" setup "$@"
            else "$bin_dir/removent-relay" setup "$@"; fi
            ;;
        *) as_root "$bin_dir/removent-relay" setup "$@" ;;
    esac
}
case "$platform" in macos-*) management_prefix= ;; *) management_prefix='sudo ' ;; esac
if [ "$setup" = yes ]; then
    if [ "$#" -gt 0 ]; then
        setup_relay "$@"
    elif [ -r /dev/tty ] && ( : </dev/tty ) 2>/dev/null; then
        setup_relay </dev/tty
    else
        echo "Run ${management_prefix}removent-relay setup --address removent://YOUR_HOST:48700 to configure the service."
    fi
fi
echo "Existing configuration and credentials are preserved. After upgrading a running service, run: ${management_prefix}removent-relay restart"

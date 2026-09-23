#!/usr/bin/env bash
# Run from a source checkout. Configuration is data, never executable shell code.
set -euo pipefail
set +x
umask 077
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
ACTION="${1:-deploy}"
if (($#)); then shift; fi
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/removent-relay"
while (($#)); do
    case "$1" in
        --config-dir) [[ $# -ge 2 ]] || { echo 'Missing --config-dir value' >&2; exit 2; }; CONFIG_DIR="$2"; shift 2 ;;
        *) echo "Unknown argument: $1" >&2; exit 2 ;;
    esac
done
case "$ACTION" in
    help|--help|-h)
        cat <<'HELP'
Usage: bash scripts/deploy_relay.sh [configure|deploy|start|stop|status|logs] [--config-dir DIR]
Default: deploy (interactive setup if no configuration exists).
VPS: Linux + Docker Compose v2. Deploy builds and starts the Rust QUIC relay.
Cloudflare: Node 24 + Docker + Wrangler login. Deploy leaves the relay stopped;
run start explicitly. stop persists across host retries and Worker restarts.
Edit deployment.json / credentials.json, then deploy to apply changes.
Configuration defaults to ~/.config/removent-relay and must be outside the checkout.
HELP
        exit 0 ;;
    configure|deploy|start|stop|status|logs) ;;
    *) echo "Unknown action: $ACTION" >&2; exit 2 ;;
esac
command -v python3 >/dev/null || { echo 'Install Python 3.11+ first' >&2; exit 1; }
python3 -c 'import sys; assert sys.version_info >= (3,11), "Python 3.11+ required"'
CONFIG_DIR="$(python3 "$ROOT/scripts/relay_setup.py" prepare "$CONFIG_DIR")"
# Serialize configuration, credential rotation and lifecycle operations.
LOCK="$CONFIG_DIR/.deploy-lock"
mkdir "$LOCK" 2>/dev/null || { echo "Deployment is already in progress (lock: $LOCK)" >&2; exit 1; }
trap 'rmdir "$LOCK"' EXIT
if [[ ! -f "$CONFIG_DIR/deployment.json" && "$ACTION" != configure && "$ACTION" != deploy ]]; then
    echo "Run configure or deploy first" >&2; exit 1
fi
if [[ "$ACTION" == configure || ! -f "$CONFIG_DIR/deployment.json" ]]; then
    python3 "$ROOT/scripts/relay_setup.py" configure "$CONFIG_DIR"
    python3 "$ROOT/scripts/relay_setup.py" render "$CONFIG_DIR"
    if [[ "$ACTION" == configure ]]; then exit 0; fi
fi
BACKEND="$(python3 "$ROOT/scripts/relay_setup.py" backend "$CONFIG_DIR")"
RUNTIME="$CONFIG_DIR/runtime"
if [[ "$BACKEND" == vps ]]; then
    [[ "$(uname -s)" == Linux ]] || { echo 'VPS deployment requires Linux (host networking preserves source IPs)' >&2; exit 1; }
    command -v docker >/dev/null || { echo 'Install Docker Engine and Docker Compose v2 first' >&2; exit 1; }
    docker compose version >/dev/null 2>&1 || { echo "Install the Docker Compose v2 plugin first" >&2; exit 1; }
    if [[ "$ACTION" == deploy ]]; then
        python3 "$ROOT/scripts/relay_setup.py" render "$CONFIG_DIR"
        docker compose -f "$RUNTIME/compose.json" build
        # Recreate after config/credential changes; keep the persistent identity volume.
        docker compose -f "$RUNTIME/compose.json" up -d --force-recreate --wait --wait-timeout 30
        python3 "$ROOT/scripts/relay_setup.py" record-deployment "$CONFIG_DIR"
        docker compose -f "$RUNTIME/compose.json" logs --tail 20 relay
        echo 'Relay started. Copy its verified fingerprint into the generated relay profiles.'
    else
        [[ -f "$RUNTIME/compose.json" ]] || { echo 'Run deploy first' >&2; exit 1; }
        case "$ACTION" in
            start) docker compose -f "$RUNTIME/compose.json" start ;;
            stop) docker compose -f "$RUNTIME/compose.json" stop ;;
            status) docker compose -f "$RUNTIME/compose.json" ps -a ;;
            logs) docker compose -f "$RUNTIME/compose.json" logs --tail 100 relay ;;
        esac
    fi
else
    if [[ "$ACTION" == deploy ]]; then
        command -v node >/dev/null || { echo 'Install Node 24+ first' >&2; exit 1; }
        node -e 'if (Number(process.versions.node.split(".")[0]) < 24) process.exit(1)'
        command -v docker >/dev/null || { echo 'Install Docker first (Cloudflare image builds require it)' >&2; exit 1; }
        docker info >/dev/null
        # Close existing tunnels with the previous deployed admin credential before
        # rotating credentials or networks. No old deployment means no stop call.
        if [[ -f "$CONFIG_DIR/deployed-origin" ]]; then
            IFS= read -r ORIGIN < "$CONFIG_DIR/deployed-origin"
            python3 "$ROOT/scripts/relay_cloudflare.py" stop "$ORIGIN" --credential-file "$CONFIG_DIR/deployed-admin.token"
        fi
        python3 "$ROOT/scripts/relay_setup.py" render "$CONFIG_DIR"
        cd "$ROOT/deploy/cloudflare"
        npm ci
        npm run check
        WRANGLER_SEND_METRICS=false npx --no-install wrangler deploy --config "$RUNTIME/wrangler.json" --secrets-file "$RUNTIME/cloudflare-secrets.json"
        python3 "$ROOT/scripts/relay_setup.py" record-deployment "$CONFIG_DIR"
        IFS= read -r ORIGIN < "$CONFIG_DIR/deployed-origin"
        python3 "$ROOT/scripts/relay_cloudflare.py" stop "$ORIGIN" --credential-file "$CONFIG_DIR/deployed-admin.token"
        printf 'Cloudflare relay deployed and stopped. Start with: bash scripts/deploy_relay.sh start --config-dir %q\n' "$CONFIG_DIR"
    elif [[ "$ACTION" == logs ]]; then
        echo 'Use Cloudflare Container logs in the dashboard.'
    else
        [[ -f "$CONFIG_DIR/deployed-origin" ]] || { echo 'Run deploy first' >&2; exit 1; }
        IFS= read -r ORIGIN < "$CONFIG_DIR/deployed-origin"
        python3 "$ROOT/scripts/relay_cloudflare.py" "$ACTION" "$ORIGIN" --credential-file "$CONFIG_DIR/deployed-admin.token"
    fi
fi

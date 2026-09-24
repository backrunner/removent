# Cloudflare Containers relay

For interactive Bash deployment, generated private configuration, credential management
and IPv4/IPv6 source allowlists, see [quick deployment](relay-quick-deploy.md).

The Rust relay now has a WebSocket carrier for Cloudflare Containers. Both Macs
configure `removent://YOUR_WORKER:443` with `transport = "websocket"` (desktop:
**Cloudflare / HTTPS**). Internally the carrier connects to the Worker's
`/v1/tunnel` over WSS; a Worker authenticates the role/room and
routes them to one Rust container. Binary messages carry the original encrypted
RVP/QUIC packets. Native pairing, device certificate pins and capability checks
remain end to end. The original QUIC/UDP VPS deployment remains available.

## Start, sleep and stop

| Action/state | Behavior |
| --- | --- |
| First deployment | Disabled; no incoming tunnel may start the container |
| Admin `start` | Persist enabled state and start the container, waiting for its TCP listener |
| Active controller | Keep running, even if no mouse movement or screen changes occur |
| No controllers for 300 seconds | Rust exits, dropping the host tunnel and releasing container resources |
| Controller connects while enabled/sleeping | Wake the container; wait up to 40 seconds for the daemon to reconnect |
| Host reconnect or admin `status` | Never start a sleeping/stopped container |
| Admin `stop` | Persist disabled state, cancel a pending cold start, destroy the container and disconnect tunnels |
| Request/retry after manual stop | Rejected until an administrator explicitly starts it again |

The enabled flag lives in Durable Object storage and survives eviction, container
restarts and redeployment with the same binding/instance identity. A stopped
container cannot be resurrected by automatic host retries. Start/stop/upgrade
operations are serialized, and stop closes admission immediately. The forwarding
path uses the low-level TCP-port API, which does not implicitly start a container
if it exits between an admission check and a WebSocket upgrade.

`RELAY_IDLE_SECONDS` configures the no-controller timeout (60–86400 seconds).
Set it to `"0"` for explicit start/stop only. Host presence and heartbeat traffic
do not prevent sleep. Stopping compute does not remove the Worker/Durable Object
or make their request/storage/network charges disappear.

## Deploy and upgrade

Use a source checkout on the management computer with Python 3.11+, Node 24+,
Docker capable of building `linux/amd64`, and a Cloudflare account with Containers
enabled. The native `removent-relay` CLI is not required for Cloudflare management.

1. Authenticate Wrangler from `deploy/cloudflare`:

   ```sh
   npm ci
   npx wrangler login
   ```

2. From the repository root, run the setup wizard and choose Cloudflare:

   ```sh
   bash scripts/deploy_relay.sh configure
   ```

   Configuration defaults to `$XDG_CONFIG_HOME/removent-relay`, or
   `~/.config/removent-relay`. Use `--config-dir /PRIVATE/PATH/relay` on every
   command for a separate deployment. The wizard creates independent host,
   client, and admin credentials in private files without printing raw tokens.
   `deployment.json` holds the address, room, resource limits, idle timeout, and
   source allowlists; `credentials.json` holds credentials. Directories use 0700
   and private files 0600. Do not edit generated `runtime/*` files.

3. Build and deploy, then explicitly start the relay:

   ```sh
   bash scripts/deploy_relay.sh deploy
   bash scripts/deploy_relay.sh start
   bash scripts/deploy_relay.sh status
   ```

   Deployment builds the Rust container, deploys the Worker and Container binding,
   stores role hashes as Worker secrets, and leaves admission **stopped**. Initial
   image provisioning may take several minutes. The script closes existing
   admission with the previous admin credential before an upgrade or credential
   change. Review the generated host/client profiles after changing configuration.

4. Securely transfer `relay-host.toml` from the private configuration directory
   to the host Mac's daemon data directory and restart the daemon. Use the
   generated `relay-client.toml` for the controller, adding the verified target
   host certificate fingerprint. Keep profiles mode 0600. The desktop can instead
   use **Add connection → Removent → Connect through relay → Cloudflare / HTTPS**;
   its credential is stored in Keychain.

To upgrade, check out the desired source release, retain the same private
configuration directory, and run `deploy` followed by `start` again. Containers
update through image deployment; `serve-container` does not run the native relay
automatic updater. Container disks are ephemeral, so use Cloudflare's dashboard
for log retention and inspection. Use the deployment script or management API
below for a persistent stop; infrastructure controls alone do not set the relay's
Durable Object admission state.

The default `standard-2` allocation supplies one vCPU and **6 GiB** provisioned
memory. `max_instances = 1` keeps room peers on the same instance. Cloudflare
bills memory/disk by provisioned size while running and CPU by active use. The
`basic` allocation is a lower-cost candidate, but measure its throughput and
latency before changing the preset. See [performance measurements](relay-performance-2026-09-20.md),
[limits](https://developers.cloudflare.com/containers/platform/limits/), and
[pricing](https://developers.cloudflare.com/containers/platform/pricing/).

The daemon's retry loop can remain running while the container is stopped or
sleeping. It re-registers within its 30-second maximum retry interval after
startup. A controller's WSS connection has a bounded 90-second total startup
deadline, including cold start and host registration. Wrong credentials,
unavailable hosts, or provisioning failures still produce a failed connection.

## Credential-free device registration

The initial relay wire protocol is v1 and uses the current device-proof and
replay-protection implementation, with no earlier development compatibility. On each Mac,
run `removent-cli identity` in the app's data directory to obtain its **public key**
and **certificate fingerprint**. For a role without an entered token, omit its
`RELAY_*_TOKEN_SHA256` secret (or use an empty value) and configure
`RELAY_HOST_PUBLIC_KEYS` / `RELAY_CLIENT_PUBLIC_KEYS` as comma-separated, full
64-hex Ed25519 public keys. A role must have at least one key if its hash is absent.
With both configured, credential **and** device identity must match. Host and
controller keys must be distinct. The admin hash remains mandatory and separate.

The Worker passes these allowlists into Rust on every boot. Device private keys
never leave their Macs. The desktop's optional credential field may then remain
empty. WSS trusts Cloudflare's TLS termination for outer routing admission;
Cloudflare and the relay still see only encrypted inner RVP payloads. The target
host certificate pin and native pairing/grants are independent of relay admission.
See [three-party trust and revocation](private-relay.md#three-independent-trust-decisions).

## Manage from the repository

Use the admin token file rather than putting a bearer token in arguments, a URL
or shell history:

```sh
python3 scripts/relay_cloudflare.py start https://YOUR_WORKER \
  --credential-file "$HOME/.config/removent/relay-admin.token"
python3 scripts/relay_cloudflare.py status https://YOUR_WORKER \
  --credential-file "$HOME/.config/removent/relay-admin.token"
python3 scripts/relay_cloudflare.py stop https://YOUR_WORKER \
  --credential-file "$HOME/.config/removent/relay-admin.token"
```

The corresponding HTTPS API is `POST /admin/start`, `POST /admin/stop` and
`GET /admin/status`, with `Authorization: Bearer ADMIN_TOKEN`. Start/status/stop
return `{ "state": "running|starting|sleeping|stopped", "enabled": bool,
"running": bool }`. Running means the container process is running; it does not
prove a host is registered or that the host has macOS capture/input permissions.
Host and client tokens cannot operate these endpoints. The helper refuses
redirects so the admin credential cannot be forwarded elsewhere.

Stopping interrupts existing sessions. After a later restart the daemon
reconnects automatically; a disconnected desktop viewer must reconnect. To rotate
credentials, stop with the current admin credential, deploy new distinct hashes,
update the affected private profiles/admin file, then start with the new admin
credential. Keeping the same Worker/DO binding preserves the disabled flag.

## Transport and resource bounds

WSS uses normal CA and hostname verification at the Cloudflare edge. Omit
`server_fingerprint`, which belongs to the QUIC relay's pinned self-signed
certificate. Plain `ws://` profiles are accepted only on loopback for local
tests. Credentials travel in HTTP headers, never query parameters. The edge and
Rust backend both validate role/room credentials. Every client sends a signed
admission request bound to the exact WSS audience, room, role, timestamp (±120 s)
and random nonce. The edge verifies Ed25519 before contacting the DO; the DO
atomically claims the nonce in a durable, expiring ledger (512 entries max)
before starting the container. Replays cannot trigger another wake. Rust then
requires a new server challenge signature before registering any route, so an
edge request cannot be replayed to take a host slot after a container restart.
Keep endpoint clocks synchronized. Authentication never runs on the media path. Only one fixed Durable Object identity can be selected.

Container disks are ephemeral. This mode has no relay identity/key files:
configuration hashes are injected on every start, the edge manages public TLS,
and end-device identities stay on the Macs. The enabled flag and bounded admission replay ledger use durable
storage. The Rust process runs as an unprivileged user without outbound internet
access, and its image contains no credentials.

Rust forwards reference-counted binary buffers without decoding screens. It
caps WebSocket messages to 2056 bytes, per-direction application queues to 16
messages, and queue waits to 50 ms. HTTP upgrade has a five-second deadline;
connection count, room capacity and ingress bandwidth are bounded. WebSocket
heartbeats detect dead connections. Explicit shutdown cancels tunnel tasks and
route registrations; a host disconnect closes its controllers.

WSS runs over TCP and therefore has head-of-line blocking on lossy links. It
is a compatibility transport, with different WAN behavior from QUIC DATAGRAM.
Socket reads and writes progress independently; a blocked video write cannot
suspend reading input in the opposite direction. See the local
[WAN latency assessment](wan-latency-2026-09-20.md) for impairment methodology
and the distinction between input delivery and visible response.
The QUIC loopback measurement does not describe Cloudflare
performance. Measure throughput, latency and loss behavior on the deployed
Container and actual network before setting a production performance target.

The new WebSocket release benchmark on this development Mac transferred 64 MiB
at **593.39 Mbps** before the v1 identifier normalization, followed by 200 small round trips with
**p50 0.240 ms, p95 0.423 ms and p99 0.507 ms**. This is loopback WS carrying real encrypted RVP,
with host/relay/controller on one machine; it excludes edge TLS, Worker routing,
Cloudflare container CPU limits, WAN loss, capture/codec/render work and concurrent
controllers. It is transport-development evidence, not an Internet service target.
The benchmark fixture uses a high bandwidth allowance; the deployment template
limits each authenticated connection to 50 MB/s (400 Mbps), with a one-second
token-bucket burst.

## Verification and platform references

The local Rust tests exercise real inner RVP device pins, handshake/control and
1 MiB media through both transports. WebSocket tests cover wrong credentials,
role/room isolation, duplicate hosts, capacity, route spoofing, cleanup, cold
start host arrival, controller-aware idle exit and rejection of untrusted TLS.
HTTP readiness requests used by the Containers SDK receive complete health
responses; malformed/text/oversized WebSocket frames are rejected.
Worker tests cover durable stop, unauthorized management, no-wake host/status
requests, controller wakeup and stop racing a cold start. The Python helper tests
private credentials and refusing credential-bearing redirects.

```sh
cargo test --locked -p removent-relay
cargo test --locked --release -p removent-relay --test tunnel \
  websocket_rvp_benchmark -- --ignored --nocapture
```

Worker type checking, lifecycle tests and Wrangler's deployment dry-run have
passed locally. Final v1 workspace regression: **315 passed, 5 ignored**; the relay suite
includes **15 passed, 2 benchmark tests ignored**. Earlier timing failures and
the successful full rerun are recorded in [the verification log](review-2026-09-20.md#initial-v1-baseline-and-beta-cleanup). Worker control tests:
**9 passed**. Management-helper tests: **2 passed**. Workspace Clippy with
`-D warnings` passed (upstream future-compatibility warnings remain).
The updated Rust binary cross-builds for Linux x86-64. The native
executable also passed a `serve-container` smoke check using environment
configuration, an HTTP readiness request and graceful SIGTERM shutdown. Docker's
daemon and a deployed Cloudflare test instance were unavailable in this workspace;
image execution, real cold starts, remote stop/restart and Cloudflare throughput
have not been accepted live. CI contains separate Linux image builds and Worker
checks. The repository supplies the implementation and deployment configuration;
no Cloudflare deployment or account mutation was performed in this change.

Cloudflare documentation checked 2026-09-20:

- [WebSocket to Container](https://developers.cloudflare.com/containers/examples/websocket/)
- [Container class and lifecycle API](https://developers.cloudflare.com/containers/reference/container-class/)
- [Low-level TCP port and destroy API](https://developers.cloudflare.com/durable-objects/api/container/)
- [Ephemeral disk and lifecycle](https://developers.cloudflare.com/containers/concepts/architecture/)
- [Secret injection](https://developers.cloudflare.com/containers/examples/env-vars-and-secrets/)
- [Instance limits](https://developers.cloudflare.com/containers/platform/limits/)

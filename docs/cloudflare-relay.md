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

## Deploy

Prerequisites: a Cloudflare account with Containers enabled, Docker capable of
building `linux/amd64`, Node 24 and Wrangler authentication. No VPS, Spectrum or
public UDP listener is required for this mode.

1. Install the prebuilt CLI using the [installer](relay-quick-deploy.md), then run `removent-relay generate-token` three times.
   Keep the **raw host**, **raw client** and **raw admin** tokens separate. Each
   is 256 random bits. Transfer raw tokens only to their respective users/Macs;
   store the admin token as one line in a private file outside this repository
   (mode 0600), for example `~/.config/removent/relay-admin.token`.
2. In [wrangler.jsonc](../deploy/cloudflare/wrangler.jsonc), set the Worker name,
   `RELAY_ROOM` and idle timeout. The default `standard-2` instance supplies one
   vCPU; `max_instances = 1` prevents room peers being split across independent
   instances. Resize after measuring the intended workload.
   This preset provisions **6 GiB**, regardless of the Rust process's much smaller
   RSS. Cloudflare bills memory/disk by provisioned size while the container is
   running, and CPU by active use. `basic` (1/4 vCPU, 1 GiB) is a lower-cost
   candidate for small deployments, but validate throughput/latency on that actual
   allocation before reducing the preset. See [performance measurements](relay-performance-2026-09-20.md)
   and Cloudflare's [limits](https://developers.cloudflare.com/containers/platform/limits/)
   and [pricing](https://developers.cloudflare.com/containers/platform/pricing/).
3. Put the three **hashes**, not raw tokens, in a private JSON secrets file:

   ```json
   {
     "RELAY_HOST_TOKEN_SHA256": "HOST_SHA256_FROM_GENERATOR",
     "RELAY_CLIENT_TOKEN_SHA256": "CLIENT_SHA256_FROM_GENERATOR",
     "RELAY_ADMIN_TOKEN_SHA256": "ADMIN_SHA256_FROM_GENERATOR"
   }
   ```

4. From `deploy/cloudflare`, run:

   ```sh
   npm ci
   npm run check
   npm test
   npm run deploy -- --secrets-file /PRIVATE/PATH/relay-secrets.json
   ```

   Wrangler builds the Rust image with the repository root as its build context,
   deploys the Worker/Container binding, and stores the hashes as Worker secrets.
   On first deployment Cloudflare may take several minutes to provision the
   image. The service is initially disabled.

5. Copy [tunnel.example.toml](../deploy/cloudflare/tunnel.example.toml) to the
   daemon's `relay-host.toml` and the controller's `relays/office.toml`, using
   the correct role token in each. Replace the URL with your Worker domain and
   `chmod 600` both real profiles. Restart the daemon. The controller can instead
   use **Add connection → Removent → Connect through relay**, select/enter the
   `removent://` address and room, and provide the target host certificate fingerprint. Its
   credential is stored in Keychain. CLI profiles require `host_fingerprint`.

The daemon's ordinary retry loop can remain running while the container is
stopped or sleeping. It will re-register within its 30-second maximum retry
interval after startup. A controller's WSS connect has a bounded 90-second total
startup deadline, including cold start and host registration. Wrong credentials,
unavailable hosts or provisioning failure still produce a failed connection.

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

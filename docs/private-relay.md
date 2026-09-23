# Private Rust relay

For interactive Bash deployment, generated private configuration, credential management
and IPv4/IPv6 source allowlists, see [quick deployment](relay-quick-deploy.md).

Removent can route its native RVP connection through a self-hosted Rust relay.
In **Add connection → Removent**, enable **Connect through relay** and enter the
room ID, relay address (`removent://host:port`), optional credential, and target
host certificate fingerprint. Select **Cloudflare / HTTPS** for a WebSocket carrier
(usually port 443), or **VPS / QUIC** (usually port 48700). Saved relay routes can
be selected again. QUIC also requires the relay certificate fingerprint; HTTPS
uses CA/hostname checks. Credentials are saved in the macOS Keychain, never in
bookmark JSON. CLI connections use an explicit `--relay-profile FILE`.
LAN discovery and direct connections continue to work without a relay.

For **Cloudflare Containers with on-demand startup, idle sleep and authenticated
manual start/stop**, use the [WSS container deployment](cloudflare-relay.md).
The QUIC deployment below is the VPS/UDP option.

See the [resource and screen-compression measurements](relay-performance-2026-09-20.md)
for reproducible per-process CPU/RSS benchmarks and WAN compression findings.
The relay executable defaults to at most two Tokio I/O workers (one on a
single-core allocation). `TOKIO_WORKER_THREADS` explicitly overrides this for a
measured workload. Idle connections do not allocate an OS thread each.

## Trust and transport

Two outbound QUIC connections traverse NAT: host → relay and controller → relay.
QUIC DATAGRAM frames carry the original RVP UDP packets. RVP's mutual TLS,
certificate identity, SPAKE2 pairing, capability grants and admission checks stay
end to end. The relay sees traffic timing, sizes, room names and endpoint IPs;
it cannot read the screen, keystrokes, clipboard or RVP credentials. Relay access
does not grant remote desktop access. Pair devices and approve their grants
before leaving a host unattended.

Outer TLS uses a mandatory SHA-256 certificate pin, with a separate relay ALPN.
There is no certificate-verification bypass. Each room has independent host and
controller tokens (256 random bits); only SHA-256 token digests are stored by the
server. Every connection also proves possession of an Ed25519 device key by
signing a fresh 256-bit server challenge bound to the room and role. A controller
token cannot replace the host. Duplicate host registration
is rejected. Tokens are not OS account passwords. Rotate a credential by
replacing its digest and restarting the relay; that disconnects existing tunnels.
Preserve the relay identity directory across updates, or distribute a new pin.

The data path forwards reference-counted opaque buffers without transcoding or
spawning a task per packet. DATAGRAM avoids a reliable outer byte stream's head
of line blocking; the original QUIC connection handles loss and retransmission.
Packets are fragmented into frames of at most 1100 bytes to work across different
path MTUs. Reassembly is bounded to 256 packets of at most 2048 bytes, and stale
partial packets expire after two seconds. Native Quinn's normal MTU fits this
limit; arbitrary jumbo UDP packets are not supported.

TLS/authentication have a five-second deadline. QUIC Retry validates source
reachability before TLS allocation. Global connection and per-room controller
limits, 32 KiB per-connection DATAGRAM queues and per-connection bandwidth
buckets bound resources. A forwarding task waits at most 50 ms for send-buffer
space before shedding that packet; this propagates brief congestion without
letting a slow destination accumulate unlimited latency. The server never accepts a destination IP from the
controller. The host bridge only forwards to the configured loopback RVP port,
with separate local UDP sockets per controller and 30-second idle cleanup.
Apply the VPS provider's network/DDoS controls for volumetric attacks; application
limits do not prevent an attacker saturating the uplink.

## Host a relay on macOS

The same installer supports macOS 13+ on Apple Silicon and Intel with a signed,
notarized Universal binary. Run it as your normal login user. Only installing
into `/usr/local/bin` may need sudo; setup and everyday CLI commands do not.

```sh
curl -fsSL https://raw.githubusercontent.com/backrunner/removent/main/scripts/install_relay.sh | sh
removent-relay status
removent-relay logs --follow
removent-relay stop
removent-relay start
removent-relay restart
```

The user LaunchAgent starts at login and recovers after a crash. `enable` and
`disable` control future logins separately from current runtime state. Logout
stops the relay, and sleeping Macs cannot forward traffic. Keep the Mac logged
in and awake, allow the selected UDP port, and configure router forwarding when
needed. The relay does not require screen-recording or accessibility permissions.

Private configuration, profiles, persistent identity and logs are under
`~/Library/Application Support/removent-relay`. Its launchd label is
`com.alkinum.removent.relay`, separate from the desktop daemon. `check`,
`fingerprint` and `export` use the macOS directory automatically. The installer
preserves service state on upgrade; explicitly restart to use the new binary.
See [installation and configuration](relay-quick-deploy.md) for export and
advanced isolated instances.

## Deploy on a Linux VPS

Install a prebuilt binary (Linux x86_64 / ARM64, systemd 247+):

```sh
curl -fsSL https://raw.githubusercontent.com/backrunner/removent/main/scripts/install_relay.sh | sh
```

The installer verifies SHA-256 and version, atomically installs the binary, and
opens its native setup wizard. No Rust toolchain or Docker is required on the VPS.
Release assets must be present in the selected stable release; older releases
without relay assets are not supported by the installer.

Setup generates distinct host/controller credentials and pinned profiles in
`/etc/removent-relay` (directory 0700, files 0600). The unprivileged DynamicUser
service receives its configuration through systemd LoadCredential; its identity
persists in `/var/lib/removent-relay`. Allow inbound UDP **48700**, or the chosen
port, in your VPS firewall. Distribute profiles through a trusted channel.

```sh
sudo removent-relay start
removent-relay status
sudo removent-relay logs --follow
sudo removent-relay stop
sudo removent-relay restart
sudo removent-relay fingerprint
```

Reinstalling preserves credentials and identity; restart explicitly after an
upgrade. `enable` / `disable` control boot startup separately. `uninstall` removes
the service but retains configuration, identity and the binary. See the
[installation and configuration guide](relay-quick-deploy.md) for unattended
setup, source allowlists, profile export and Cloudflare management.

Native services also support signed automatic updates, disabled by default. Set
`[updates] enabled = true` in `server.toml` and restart to enable them. The default
interval is 24 hours, with an initial check after 30 seconds; installation restarts
the process and interrupts current tunnels. Use `removent-relay check-update` for
a manual check without installation. Local logs in `identity_dir/logs` rotate at
8 MiB with three backups, matching the desktop client's logging policy. See the
[update and logging settings](relay-quick-deploy.md#自动更新与手动检查).

For development, `cargo build --locked --release -p removent-relay` still builds
the same CLI. `init --dir /PRIVATE/PATH/relay --address removent://host:48700`
generates standalone configuration, and `serve /PRIVATE/PATH/relay/server.toml`
runs in the foreground. The Docker templates remain available for custom
container deployments; keep their persistent identity volume across updates.

Use a DNS-only Cloudflare record pointing to the VPS. Ordinary orange-cloud
HTTP proxying and Workers do not expose this UDP listener. Cloudflare Spectrum
with a plan supporting arbitrary UDP can proxy a VPS origin, but is not a
Workers-hosted relay. Disable Spectrum's Proxy Protocol for this raw QUIC origin.
The new WSS transport provides a separate [Cloudflare Containers path](cloudflare-relay.md).
It runs the Rust relay behind a Worker and does not require exposing this UDP
listener. Native device authentication remains inside encrypted RVP. Its TCP/WSS
performance must be measured separately from the QUIC path below.

Sources checked 2026-09-20:
[Workers sockets](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/),
[Spectrum limitations](https://developers.cloudflare.com/spectrum/reference/limitations/).

## Configure the host and controller

The installed macOS data directory is
`~/Library/Application Support/removent/userdata`; custom builds use
`REMOVENT_DATA_DIR` or the development data directory. Use the same directory as
the daemon, visible in the tray's **Open Data Directory** action.

Host: copy [tunnel.example.toml](../deploy/relay/tunnel.example.toml) to
`relay-host.toml` in that directory, fill in `server = "removent://host:48700"`,
`transport = "quic"`, `server_fingerprint`,
`room = "office"`, and the **host** token, then `chmod 600` the file. Restart the
background service. The daemon's host runner owns the outbound tunnel and retries
failures with backoff up to 30 seconds. Turning off sharing stops the tunnel;
quitting the desktop or tray does not. Tray and `daemon status` expose relay
connection state/errors separately from local listener readiness.

Controller: use the desktop form described above. Enter the configured room name
(e.g. `office`), relay URL, relay pin for QUIC, and the **target host** certificate
fingerprint obtained through a trusted channel. These are two different pins.
The optional credential is the controller token, not the host/admin token or an
OS password. Saved bookmarks retain the exact target host pin, including on
quick resume; a different RVP host is rejected even if the relay routes to it.
Editing the room, endpoint, transport or relay pin clears a loaded credential.

For the CLI, create a private profile with the same server, transport, relay pin,
room, controller token and `host_fingerprint`. `chmod 600` the profile:

```sh
removent-cli ping --relay-profile /private/path/office.toml --count 20
```

Both transports use only `removent://host:port` in configuration and the desktop.
The port is required; IPv6 uses brackets, such as `removent://[::1]:48700`.
Credentials, paths and query parameters are not accepted in the address.
The WebSocket implementation derives `wss://host:port/v1/tunnel` internally.
Older URL schemes and profile aliases are rejected without migration.

### Three independent trust decisions

| Verifier | What is verified | What it authorizes |
| --- | --- | --- |
| Both Macs | QUIC relay certificate pin, or WSS CA and hostname | Sending routing requests to this relay |
| Relay | Role-specific credential if configured, device signature, and role/room public-key allowlist if configured | Registering a host or controller route |
| Controller / host | Exact target host certificate pin / paired controller identity and capability grants | An end-to-end RVP session |

Run `removent-cli identity` on each Mac, using the app/daemon's data directory.
It prints only the public Ed25519 key and full certificate fingerprint. Exchange
these through a trusted channel. Do not use the relay's response as the initial
source of host identity trust.

For **no entered credential**, configure `host_public_keys` / `client_public_keys`
in the relay room, and omit that role's token hash (or set it to `""`). Example:

```toml
[[rooms]]
name = "office"
host_public_keys = ["HOST_ED25519_PUBLIC_KEY_64_HEX"]
client_public_keys = ["CONTROLLER_ED25519_PUBLIC_KEY_64_HEX"]
```

On the corresponding Mac, omit `token` or set it to `""`. An empty token does
**not** authorize an anonymous device: no token hash and no keys is an invalid
server configuration. If both a hash and keys are configured, **both** must pass.
Host and client allowlists cannot overlap. This mode can also restrict who may
use a valid bearer token. Device keys remain on the endpoints. To revoke a device,
remove its public key and restart the relay (or stop/redeploy/start on Cloudflare),
which also closes existing tunnels. Revoke native desktop grants separately.

The initial public protocol is **v1** (`removent-relay/1`,
`removent-relay.ws.v1`). It retains device challenge signatures, audience-bound
admission, replay protection and optional credentials backed by registered keys.
This is the current implementation with a v1 identifier; no earlier development
wire formats are supported. Pairing and grants are still required before
unattended use.

A missing profile, wrong pin, wrong role token, unavailable host or full room
fails the connection. The host bridge automatically reconnects after relay
outages. A viewer whose **outer tunnel** was lost must reconnect explicitly;
the existing RVP quick-resume path alone cannot restore that outer connection.
The relay does not discover hosts, wake a sleeping machine, start a logged-out
user's LaunchAgent or grant macOS privacy permissions.

## Verification

```sh
cargo test --locked -p removent-relay
cargo test --locked --release -p removent-relay --test tunnel \
  relay_rvp_benchmark -- --ignored --nocapture
```

The tests exercise real nested QUIC with mutual device pins, RVP handshake,
control messages and fragmented media; invalid credentials/pins/roles, duplicate
hosts, room isolation, limits and cleanup. The benchmark measures 64 MiB of
reliable inner-stream traffic and 200 small round trips through the relay on
loopback. It is a transport measurement, not a screen-to-screen latency claim.
WAN loss, VPS CPU, geography and bandwidth must be measured on the intended
deployment. Linux CI builds/tests this crate separately from the macOS UI.


### Local measurements (2026-09-20)

Before the v1 identifier normalization, this same relay implementation on the
development Mac transferred **64 MiB at
258.33 Mbps**; 200 subsequent small round trips measured **p50 0.348 ms,
p95 0.829 ms, p99 1.659 ms**. These are single-controller loopback results with
host, relay and viewer on one machine, excluding capture/codec/render work.
They do not establish concurrent-session capacity, WAN packet-loss performance
or a production VPS service-level guarantee. Benchmark source is
`crates/relay/tests/tunnel.rs::relay_rvp_benchmark`.

The first stress run exposed a DATAGRAM queue-accounting panic in
`quinn-proto 0.11.17`; the relay now requires **0.11.18 or later**, and the lockfile
was upgraded accordingly. An earlier drop-on-congestion run also measured only
29.29 Mbps with a 66.082 ms p99 under local load. Bounded send-buffer waiting is
part of the final implementation, not a reason to increase queues without limit.

The standalone binary was cross-built for `x86_64-unknown-linux-gnu` with
cargo-zigbuild and verified as an ELF executable. Docker runtime and a live VPS
were not available for deployment acceptance in this workspace; the supplied
Linux workflow covers build/test and container packaging when run in CI.

# Removent

[简体中文](README.zh-CN.md)

Removent is a remote desktop for macOS, written in Rust. Connect directly on your
LAN, or use an optional self-hosted Rust relay for private RVP connections across
networks. No hosted account is required.

- **Fast by design**: QUIC transport, VideoToolbox hardware HEVC/H.264 encoding,
  opt-in software AV1, ScreenCaptureKit capture, and Opus audio.
- **Secure by default**: mutual TLS with pinned device certificates, SPAKE2 PIN pairing,
  per-connection admission control.
- **Native feel**: GPUI-based desktop app plus a menu-bar tray for the always-on host service.

**Status**: early development (M1 milestone — discovery, pairing, ping and the media pipeline
are working; expect rough edges).

## Requirements

- macOS 13.0 or later, Apple Silicon (arm64)
- For native Removent connections, a reachable host on the LAN or a configured private relay
- For compatibility connections, a reachable VNC or RDP server

## Quick start

### Option 1: Download the DMG

Grab `Removent-<version>-macos-arm64.dmg` from the
[latest release](https://github.com/backrunner/removent/releases), open it, and double-click
**Removent.app** → **Install and Open**, or drag it into **Applications**. The app is Developer-ID signed and notarized, so it
opens without Gatekeeper warnings.

### Option 2: One-line installer

```bash
curl -fsSL https://raw.githubusercontent.com/backrunner/removent/main/scripts/install.sh | bash
```

### First launch

macOS will ask for two permissions — both are required for hosting a session:

- **Screen Recording** — to share this Mac's screen.
- **Accessibility** — to inject keyboard/mouse input when this Mac is being controlled.

Grant them under *System Settings → Privacy & Security*, then toggle the host service on from
the menu-bar tray. On the other Mac, find this device in the device list (or connect by IP),
enter the pairing PIN shown on the host, and you're in.

### LAN discovery

Settings → **LAN discovery** controls automatic discovery separately for Removent,
VNC and RDP. Removent is enabled by default; VNC and RDP are opt-in. Changes apply
immediately and persist across restarts. Turning discovery off removes that
protocol's nearby entries without changing saved connections, active sessions or
this Mac's sharing service.

Discovery combines Bonjour/mDNS (`_removent._udp`, `_rfb._tcp`, `_rdp._tcp`,
`_ms-wbt-server._tcp`) with active IPv4 LAN scans for VNC on TCP 5900 and RDP on
TCP 3389. This includes Windows RDP hosts that do not advertise. Custom ports
are discovered through broadcasts or can be entered manually; IPv6 discovery
uses mDNS.

Scans visit directly connected subnets, exclude this machine and point-to-point
VPN interfaces, and confirm RFB/RDP handshakes without logging in. Each protocol
uses at most 32 concurrent probes with a 1.2-second deadline. Each round visits
up to 256 new addresses per subnet, starting near this machine, then waits 30
seconds. On larger networks, the local /24 is also revisited each round while
the rest is covered progressively; known servers are checked every round and
removed when unreachable. Disabling a protocol cancels its
probes. Broadcast and scanned entries for the same endpoint merge, preferring
the advertised name.

Nearby entries show their protocol; connecting to a discovered VNC/RDP service
opens its credential form with the address and port already filled in.

### Apple Remote Desktop / VNC compatibility

In addition to the native RVP/QUIC protocol, the host can optionally expose a standard RFB/VNC
listener for VNC viewers. It is disabled by default. Enable
**Apple Remote Desktop / VNC** in Settings, set a password, and restart the host service; clients
can then connect over TCP port `5900`. An empty password selects RFB's unauthenticated mode and is
suitable only for an isolated, trusted LAN. The compatibility layer provides a raw framebuffer and
keyboard/mouse input; Removent pairing, clipboard, audio, and adaptive media remain available only
through RVP.

To use Removent as a VNC viewer, choose **Add connection**, select VNC, then enter the host, port
(default `5900`), and credentials for that connection. Standard VNC servers only need a password.
Apple Remote Desktop / macOS Screen Sharing uses a macOS username and password with Apple
type-30 Diffie-Hellman/AES authentication. Hostnames, IPv4, IPv6, and custom ports are
supported; the protocol is selected explicitly instead of inferred from the port.

The viewer negotiates RFB 3.3/3.7/3.8 and supports Raw, CopyRect, Hextile, and desktop resizing.
Authentication currently supports None (1), VNC password (2), and Apple ARD (30).
Apple type 35, VeNCrypt/TLS, RealVNC proprietary authentication, and UltraVNC MS-Logon
are not implemented; servers must offer a supported method. See the
[compatibility review](docs/review-2026-09-16.md) for verification and limits.

VNC sends input independently of framebuffer processing. Consecutive pointer moves retain the
latest position, while keys, clicks, and scrolls stay ordered through temporary stalls. Performance
info is hidden by default: move to the top window edge and click the toolbar's gauge icon, or press
`Ctrl+Cmd+I`, to toggle a translucent panel at the upper left. It shows resolution, frame rate,
session duration, and VNC receive rate, pixel processing time, and input queue measurements.
Input send time is measured locally and excludes the remote response.

### RDP compatibility

**Add connection** opens a two-level dialog: choose Removent, VNC, or RDP, then enter connection
details. Back or Esc returns to protocol selection, and pending connections can be cancelled.
RDP defaults to port `3389` and accepts a username, password, and optional domain. IronRDP provides
an in-app desktop session with TLS, NLA/CredSSP, graphics, keyboard, and mouse input.

Server certificates are verified by default. For self-signed certificates, use system trust or
explicitly allow untrusted certificates for that connection. Dialog passwords are not written to
settings. RDP support is currently client-only, without audio, clipboard, file redirection, or
automatic reconnection. Windows hosts must have Remote Desktop enabled and allow the account to log in.

## Unattended hosting

The packaged server runs independently under launchd. Quitting the main window or
menu bar app leaves it running. **Start Server at Login** and **Show Menu Bar App
at Login** are separate options; disabling sharing is persisted across restarts.
Complete daemon privacy setup and device pairing before relying on unattended
reconnections. Hosting requires a logged-in graphical session; FileVault preboot
unlock and the initial login window are outside the current host's support.

The bundle includes `Contents/MacOS/removent-cli` with `daemon start`,
`daemon login-on` and `daemon status`. See the [macOS background service guide](docs/macos-background-service.md)
for installation, server-only commands, permissions and verification.

## Private relay and macOS unlock

Install a prebuilt private relay on Linux x86_64 / ARM64 or macOS 13+ (Apple Silicon / Intel):

```sh
curl -fsSL https://raw.githubusercontent.com/backrunner/removent/main/scripts/install_relay.sh | sh
```

The installer verifies the release binary and opens its setup wizard. Manage it
with `removent-relay start / stop / restart / status / logs`. Linux uses systemd
247+ (sudo for changes); macOS uses a user LaunchAgent (no sudo, starts at login
and stops at logout). Neither needs Rust or Docker on the relay machine. Requires
a stable release containing relay assets. See [installation, upgrades and Cloudflare
management](docs/relay-quick-deploy.md).

[Deploy the Rust relay](docs/private-relay.md) on a VPS, configure separate host/controller
credentials (or registered device keys), then select the relay in **Add connection**
and enter `removent://host:port` and the target host fingerprint. Select
Cloudflare / HTTPS or VPS / QUIC separately. The current protocol is v1. Credentials stay in Keychain. The
daemon maintains the host tunnel independently of the desktop and tray.

[Cloudflare Containers](docs/cloudflare-relay.md) use the Rust WSS relay with
authenticated start/stop/status commands and controller-aware idle sleep. Manual
stop persists; background daemon retries cannot restart the container.

Native sessions adapt bitrate and frame rate from sender pressure and automatic
receiver feedback, preserving capture dimensions and a minimum per-frame detail
budget. Constrained links favor readable text over motion smoothness. See the
[quality measurements and limits](docs/readable-quality-2026-09-20.md).

[macOS unlock investigation](docs/macos-unlock.md) distinguishes a locked user
session, the login window and FileVault. Apple silicon with macOS 26+ has an
Apple-supported SSH FileVault unlock path after restart; it is separate from RVP
and is not implemented by the relay. Locked-Mac acceptance is still required.

## Website

The official website source lives in [`website/`](website/README.md), built with svedocs.
It includes a custom landing page, English and Chinese documentation, and local search.
See the website README for development, validation, and static hosting instructions.

## Build from source

Requires a stable Rust toolchain (1.89+, required by IronRDP) and full Xcode with Swift and the Metal toolchain.

```bash
git clone https://github.com/backrunner/removent.git
cd removent

# Build incrementally and start the app, daemon and tray
scripts/dev.sh

# Restart immediately using existing debug binaries
scripts/dev.sh --no-build

# Or package a .app bundle + zip into dist/ (ad-hoc signed for local use)
scripts/package.sh
```

The development script uses `userdata/dev` (override with `REMOVENT_DATA_DIR`),
skips scheduled update checks, and stops its own processes on Ctrl-C. Use
`--no-tray` for app/daemon work, `--build-only` to build without launching, or
`--help` for options. The first build requires compiling dependencies; subsequent
runs build incrementally. See [the review report](docs/review-2026-09-06.md) for
findings, validation and performance measurements.

Run tests with `cargo test --workspace`; the tray integration test is
`bash tray/Tests/run_integration_test.sh`.

### Benchmarks

Release-mode codec and control-path measurements are reproducible with:

```bash
cargo run --release -p removent-media-codec --example benchmark
cargo run --release -p removent-net --example rtt_benchmark
```

The codec benchmark reports H.264/HEVC/AV1 encode cost, per-frame encode-to-decode latency,
throughput, compression, static-frame skip rate, and Opus encode/decode cost. The RVP benchmark reports QUIC
`Ping/Pong` control RTT percentiles. Both commands print the build mode and environment.

### Codec and static-frame policy

The host compares complete BGRA frames and skips a frame only when its pixels are
byte-for-byte identical to the last frame successfully written to the video stream. A
keyframe request always bypasses this check, and the most recent frame is cached so a
request can be served even while the desktop is idle. H.264, HEVC, and AV1 also compress
unchanged regions through inter-frame prediction; exact full-frame deduplication avoids the
encoder input copy, encode call, and network packet when the entire image is unchanged.

RVP codec id `0x03` carries complete AV1 temporal-unit OBU payloads. Both peers advertise the
software AV1 capability, but HEVC remains the default because real-time software AV1 has a
substantially higher CPU and latency cost. Set `REMOVENT_VIDEO_CODEC=av1` in the host/daemon
environment to opt in; a failed pre-session encoder probe falls back to HEVC. The current AV1
path uses rav1e for software encoding and rav1d for software decoding. The benchmark also prints
VideoToolbox hardware support so a hardware AV1 backend can be selected in a future revision.

## How it works

Three processes cooperate:

- **Removent.app** (`crates/app`) — the GPUI user interface and controller-side engine
  (decode, render, input forwarding).
- **removentd** (`crates/daemon`) — a headless always-on daemon hosting the controlled-side
  pipeline (capture, encode, input injection), managed by the tray over a Unix-socket IPC.
- **RemoventTray** (`tray/`, Swift) — the menu-bar app: service toggle, pairing PIN display,
  admission prompts.

The wire protocol ("RVP") runs over QUIC with mutual TLS and Ed25519 device identities.
See [`.agents/`](.agents/README.md) for the full design documents (requirements,
architecture, protocol — currently in Chinese).

## Updates

Removent checks for updates 30 seconds after launch and then every 24 hours.
The updater uses GitHub's
`releases/latest/download/latest.json` endpoint, which excludes prereleases and
returns 404 until a stable release exists. You can turn checks off in the app's
settings; they never interfere with LAN-only operation.

Updates are **notify-only** — a badge appears in the settings page and the menu bar, and
nothing is installed until you ask. When you do update, the download is verified against
the manifest's SHA-256 and an Ed25519 signature (the public key is compiled into the app)
before anything is swapped in, and the previous version is kept for rollback.

## Internationalization

The UI is available in English and Simplified Chinese, following the system language by
default; you can override it in the app's settings. Code comments and docs for contributors
are in English.

## Releasing (maintainers)

Tag a version matching `Cargo.toml` (`v0.1.2`) and push — `.github/workflows/release.yml`
builds, Developer-ID signs, notarizes, and attaches the DMG/zip/`latest.json` to a GitHub
release. Required secrets are documented at the top of that workflow file. To cut a release
locally, set `APPLE_SIGNING_IDENTITY` plus notarization credentials
(see `scripts/notarize.sh`) and run `scripts/release.sh`.

## License

[Apache-2.0](LICENSE)

Only stable SemVer releases are accepted. See the [release guide](docs/release-pipeline.md) for signing and publishing.

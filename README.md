# Removent

[简体中文](README.zh-CN.md)

Removent is a LAN remote desktop for macOS, written in Rust. It lets you view and control
another Mac on the same local network — no relay servers, no accounts, no traffic leaving
your LAN.

- **Fast by design**: QUIC transport, VideoToolbox hardware HEVC/H.264 encoding,
  opt-in software AV1, ScreenCaptureKit capture, and Opus audio.
- **Secure by default**: mutual TLS with pinned device certificates, SPAKE2 PIN pairing,
  per-connection admission control.
- **Native feel**: GPUI-based desktop app plus a menu-bar tray for the always-on host service.

**Status**: early development (M1 milestone — discovery, pairing, ping and the media pipeline
are working; expect rough edges).

## Requirements

- macOS 13.0 or later, Apple Silicon (arm64)
- For native Removent connections, both Macs on the same LAN (mDNS discovery)
- For compatibility connections, a reachable VNC or RDP server

## Quick start

### Option 1: Download the DMG

Grab `Removent-<version>-macos-arm64.dmg` from the
[latest release](https://github.com/backrunner/removent/releases), open it, and drag
**Removent.app** into **Applications**. The app is Developer-ID signed and notarized, so it
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
type-30/type-35 Diffie-Hellman/AES authentication. Hostnames, IPv4, IPv6, and custom ports are
supported; the protocol is selected explicitly instead of inferred from the port.

### RDP compatibility

**Add connection** opens a two-level dialog: choose Removent, VNC, or RDP, then enter connection
details. Back or Esc returns to protocol selection, and pending connections can be cancelled.
RDP defaults to port `3389` and accepts a username, password, and optional domain. IronRDP provides
an in-app desktop session with TLS, NLA/CredSSP, graphics, keyboard, and mouse input.

Server certificates are verified by default. For self-signed certificates, use system trust or
explicitly allow untrusted certificates for that connection. Dialog passwords are not written to
settings. RDP support is currently client-only, without audio, clipboard, file redirection, or
automatic reconnection. Windows hosts must have Remote Desktop enabled and allow the account to log in.

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

Removent checks for updates 30 seconds after launch and then every 24 hours by fetching
[`latest.json`](https://github.com/backrunner/removent/releases/latest/download/latest.json)
from GitHub Releases. You can turn this off in the app's settings; the check never
interferes with LAN-only operation.

Updates are **notify-only** — a badge appears in the settings page and the menu bar, and
nothing is installed until you ask. When you do update, the download is verified against
the manifest's SHA-256 and an Ed25519 signature (the public key is compiled into the app)
before anything is swapped in, and the previous version is kept for rollback.

## Internationalization

The UI is available in English and Simplified Chinese, following the system language by
default; you can override it in the app's settings. Code comments and docs for contributors
are in English.

## Releasing (maintainers)

Tag a version matching `Cargo.toml` (`v0.1.0`) and push — `.github/workflows/release.yml`
builds, Developer-ID signs, notarizes, and attaches the DMG/zip/`latest.json` to a GitHub
release. Required secrets are documented at the top of that workflow file. To cut a release
locally, set `APPLE_SIGNING_IDENTITY` plus notarization credentials
(see `scripts/notarize.sh`) and run `scripts/release.sh`.

## License

[Apache-2.0](LICENSE)

Beta builds check for later betas and stable releases; stable builds only check stable releases. See the [release guide](docs/release-pipeline.md) for signing and publishing.

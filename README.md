# Removent

[简体中文](README.zh-CN.md)

Removent is a LAN remote desktop for macOS, written in Rust. It lets you view and control
another Mac on the same local network — no relay servers, no accounts, no traffic leaving
your LAN.

- **Fast by design**: QUIC transport, VideoToolbox hardware HEVC/H.264 encoding,
  ScreenCaptureKit capture, Opus audio.
- **Secure by default**: mutual TLS with pinned device certificates, SPAKE2 PIN pairing,
  per-connection admission control.
- **Native feel**: GPUI-based desktop app plus a menu-bar tray for the always-on host service.

**Status**: early development (M1 milestone — discovery, pairing, ping and the media pipeline
are working; expect rough edges).

## Requirements

- macOS 13.0 or later, Apple Silicon (arm64)
- Both Macs on the same LAN (devices discover each other via mDNS)

## Quick start

### Option 1: Download the DMG

Grab `Removent-<version>-macos-arm64.dmg` from the
[latest release](https://github.com/removent/removent/releases/latest), open it, and drag
**Removent.app** into **Applications**. The app is Developer-ID signed and notarized, so it
opens without Gatekeeper warnings.

### Option 2: One-line installer

```bash
curl -fsSL https://raw.githubusercontent.com/removent/removent/main/scripts/install.sh | bash
```

### First launch

macOS will ask for two permissions — both are required for hosting a session:

- **Screen Recording** — to share this Mac's screen.
- **Accessibility** — to inject keyboard/mouse input when this Mac is being controlled.

Grant them under *System Settings → Privacy & Security*, then toggle the host service on from
the menu-bar tray. On the other Mac, find this device in the device list (or connect by IP),
enter the pairing PIN shown on the host, and you're in.

## Build from source

Requires a stable Rust toolchain (1.85+) and Xcode command line tools with Swift.

```bash
git clone https://github.com/removent/removent.git
cd removent

# Run the app in debug mode
cargo run -p removent-app

# Or package a .app bundle + zip into dist/ (ad-hoc signed for local use)
scripts/package.sh
```

Run tests with `cargo test --workspace`; the tray integration test is
`bash tray/Tests/run_integration_test.sh`.

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
[`latest.json`](https://github.com/removent/removent/releases/latest/download/latest.json)
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

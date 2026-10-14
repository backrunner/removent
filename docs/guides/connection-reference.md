# Connection and media reference

For installation and a first connection, start with the [README](../../README.md).

## Unattended connection authentication

Choose **Settings → Security → Connection authentication** on the desktop host.
The iPhone/iPad app and desktop controller follow the host's choice automatically:

- **System account** (default): use the username and login password of the account running Removent on the Mac host. System passwords are not stored in Removent settings.
- **Pairing code**: enter the six-digit code displayed on the host. Choose **Remember paired devices** or **Pair every connection** in desktop Security settings.
- **Access password**: set a non-empty host password; every new session requires it, including previously paired devices.
- **OTP**: copy the setup secret into an authenticator as TOTP / SHA-1 / 6 digits / 30 seconds, then enter its current code. Each code can be used once; adjacent clock steps are tolerated.
- **No authentication**: anyone who can reach the host can connect and control it without credentials.

System account, password, OTP and no authentication accept connections without a local approval prompt.
Turn off **Allow connections** to deny access. Policy changes restart the sharing listener,
end active sessions and invalidate old resume tokens. Removent access passwords and OTP secrets remain in the host's private
`settings.toml` (0600); they do not sync through iCloud. Relay routing credentials remain separate,
and the restricted LoginWindow service still requires previously approved device grants.
System account, password and OTP require updated controllers; the pairing-code wire format remains compatible.

## Connection by code and CLI

The host can generate a **12-digit connection code** from desktop Security settings or the CLI.
Enter that code in the controller's address field to find and pair with the computer on the same
LAN. Through a relay, select the same relay and enter its access password if required, then enter the code
instead of a room name; the host fingerprint is established by successful pairing.
A generated code expires after five minutes and is consumed by one successful connection.
Generating a code authorizes that connection without a second local approval prompt.
The host must use pairing-code authentication, be sharing, allow new-device pairing, and have no active session when generating.

```sh
removent-cli pairing generate  # Generate a connection code
removent-cli pairing show      # Show the current connection code or pending six-digit PIN
removent-cli pairing watch     # Print pairing-code events as they arrive
removent-cli pairing revoke    # Cancel the generated code
```

The CLI reads the local daemon's private socket. Use the same data directory as the desktop
host (`REMOVENT_DATA_DIR` when overridden). Remembered pairing binds the controller's device
certificate; **Pair every connection** also disables quick resume. Controllers save the resolved
computer address or relay room after pairing, never the temporary code. Code-only connections
require updated host, controller, and relay versions. Relay room names starting with `pair-` are reserved.

## Apple Remote Desktop / VNC compatibility

In addition to the native RVP/QUIC protocol, the host can optionally expose a standard RFB/VNC
listener for VNC viewers. It is disabled by default. Enable
**Apple Remote Desktop / VNC** in Settings and set a password; the host reloads its listener, and clients
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
[compatibility review](../validation/review-2026-09-16.md) for verification and limits.

VNC sends input independently of framebuffer processing. Consecutive pointer moves retain the
latest position, while keys, clicks, and scrolls stay ordered through temporary stalls. Performance
info is hidden by default: move to the top window edge and click the toolbar's gauge icon, or press
`Ctrl+Cmd+I`, to toggle a translucent panel at the upper left. It shows resolution, frame rate,
session duration, and VNC receive rate, pixel processing time, and input queue measurements.
Input send time is measured locally and excludes the remote response.

## RDP compatibility

**Add connection** opens a two-level dialog: choose Removent, VNC, or RDP, then enter connection
details. Back or Esc returns to protocol selection, and pending connections can be cancelled.
RDP defaults to port `3389` and accepts a username, password, and optional domain. IronRDP provides
an in-app desktop session with TLS, NLA/CredSSP, graphics, keyboard, and mouse input.

Server certificates are verified by default. For self-signed certificates, use system trust or
explicitly allow untrusted certificates for that connection. Dialog passwords are not written to
settings. RDP support is currently client-only, without audio, clipboard, file redirection, or
automatic reconnection. Windows hosts must have Remote Desktop enabled and allow the account to log in.

## Benchmarks

Release-mode codec and control-path measurements are reproducible with:

```bash
cargo run --release -p removent-media-codec --example benchmark
cargo run --release -p removent-net --example rtt_benchmark
```

The codec benchmark reports H.264/HEVC/AV1 encode cost, per-frame encode-to-decode latency,
throughput, compression, static-frame skip rate, and Opus encode/decode cost. The RVP benchmark reports QUIC
`Ping/Pong` control RTT percentiles. Both commands print the build mode and environment.

## Codec and static-frame policy

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

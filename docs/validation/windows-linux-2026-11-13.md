# Windows and Linux desktop validation

Work branch: `feat/windows-linux-desktop`, PR #2. These are development packages;
no release tag, merge, system-service installation, or public release was made.

## Observed locally

- Native Linux arm64 Debian 12 container, Xvfb 1280x800, Openbox and Mesa software rendering.
- Actual desktop pixels captured with xcap, H.264 encoded and decoded, matched against the independent red Tk window.
- Native click, Ctrl+A and exact mixed Chinese/English text received by that independent UI, including committed text and a physical-key Unicode payload.
- Authenticated pinned QUIC host/client session negotiated H.264, delivered real desktop pixels and input, synchronized text clipboard both directions, and shut down cleanly.
- Real Unix IPC: status, sharing enable/disable, pairing invitation, acknowledged shutdown, successful daemon exit, and persisted stop intent.
- Actual Xfce panel registration, tray start, sharing enable/disable, native GTK pairing dialog, service stop and tray quit. The test caught and fixed duplicate pairing dialogs and the former external `zenity` dependency.
- Visible GPUI main window content, first-frame receipt and clean exit. The test checks content pixels, not just window creation. Mesa CPU Vulkan black output was reproduced; the OpenGL preference fixed the default path.
- Native Secret Service credential create/read/update/delete, including Unicode and missing entries, with an isolated test collection.
- Linux: core 72, protocol 16, input 11, client 126, host 71 and codec 5 library tests passed. Native PAM authentication separately passed with a temporary non-root OS account (valid and invalid passwords). macOS workspace strict Clippy passed; Windows GNU cross-check passed including network and credential probes.

## Native CI

The [Desktop platforms workflow](https://github.com/backrunner/removent/actions/workflows/desktop-platforms.yml)
builds Windows x86_64 and Linux x86_64 desktop, daemon and tray. It runs native
desktop probes, IPC/admission tests and protocol/codec/session regressions before
packaging all three executables with checksums. Windows also checks password and
desktop-owner SID validation using a disposable local OS account.

The [2026-11-13 native run](https://github.com/backrunner/removent/actions/runs/43462842578)
passed both platforms and produced both archives. Windows passed 298 library
tests plus eight IPC tests and native account validation. Its captured desktop
and Explorer tray were reviewed, including caption glyphs, actual minimize and
maximize clicks, pairing dialogs, exact `hello中文` input, and both clipboard
directions through a pinned QUIC session. Both archive checksums and executable
formats were checked after download. The Windows package check additionally
launches the extracted desktop, host and tray and verifies that the desktop loads
its bundled Visual C++ runtime instead of relying on the runner's installation.

Each run attaches desktop/tray screenshots and logs to its exact commit. See
[PR #2 checks](https://github.com/backrunner/removent/pull/2/checks) for the run
results and downloadable packages. Earlier failures exposed a daemon fixture
error and Windows append-only log-lock access; both have regression coverage.

## Boundaries

The virtual desktop exercises the real capture/input APIs and real QUIC loopback
transport. It does not certify physical GPUs, WAN jitter, multi-monitor scaling,
locked/UAC secure desktops or tray behavior in every desktop shell. Linux needs
X11 for hosting and an AppIndicator-capable panel for a visible tray. Wayland
hosting, system-audio capture, pre-login access, native Computer Use and automatic
package installation remain unavailable. Unsupported capabilities are not
advertised as working. Use the account password rather than a Windows Hello PIN.

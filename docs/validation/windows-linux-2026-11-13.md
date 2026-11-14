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

## Expanded tray acceptance

`scripts/qa/windows_tray.py` and `scripts/qa/linux_tray.py` now share a real-daemon
lifecycle harness. They click the shell's actual icon, select native menu items,
and click native dialog buttons. Xfce menu labels and enabled state are read from
D-Bus, but selection uses keyboard input in the visible popup rather than invoking
its exported callbacks. Windows uses Explorer icon bounds and actual pointer input.

The harness covers duplicate tray startup, opening/restoring one real main window,
reopening pairing dialogs, desktop-first approval, competing answers, default denial,
explicit denial/approval, controller cancellation, and the real 30-second timeout.
An accepted pinned QUIC session must keep delivering decoded desktop frames after
the tray quits and after Explorer/Xfce restarts. A ten-minute CI soak records tray
RSS, handles/file descriptors and threads. Killing the daemon must dismiss pending
prompts and recover sharing; a deliberate stop must survive watchdog polling and
tray restart. Chinese dark presentation, Linux 2x presentation, and restoring a
window on the second of two logical RandR monitors run separately. Logical monitors
do not validate physical monitor hotplug or mixed-DPI transitions.

Screenshots, JSON results and daemon/desktop logs are retained in the workflow's
`desktop-evidence-*` artifacts, including on failure. Passing compilation or only
part of this harness does not establish that the whole lifecycle passed; use the
exact PR head's checks and `results.json` when evaluating a development package.

## Boundaries

The virtual desktop exercises the real capture/input APIs and real QUIC loopback
transport. It does not certify physical GPUs, WAN jitter, multi-monitor scaling,
locked/UAC secure desktops or tray behavior in every desktop shell. Linux needs
X11 for hosting and an AppIndicator-capable panel for a visible tray. Wayland
hosting, system-audio capture, pre-login access, native Computer Use and automatic
package installation remain unavailable. Unsupported capabilities are not
advertised as working. Use the account password rather than a Windows Hello PIN.

# Windows and Linux desktop validation

Work branch: `feat/windows-linux-desktop`, PR #2. These are development packages;
no release tag, merge, system-service installation, or public release was made.

## Observed locally

- Native Linux arm64 Debian 12 container, Xvfb 1280x800, Openbox and Mesa software rendering.
- Actual desktop pixels captured with xcap, H.264 encoded and decoded, matched against the independent red Tk window.
- Native click, Ctrl+A and exact text received by that independent UI.
- Authenticated pinned QUIC host/client session negotiated H.264, delivered real desktop pixels and input, synchronized text clipboard both directions, and shut down cleanly.
- Real Unix IPC: status, sharing enable/disable, pairing invitation, acknowledged shutdown, successful daemon exit, and persisted stop intent.
- Actual Xfce panel registration, tray start, sharing enable/disable, native GTK pairing dialog, service stop and tray quit. The test caught and fixed duplicate pairing dialogs and the former external `zenity` dependency.
- Visible GPUI main window content, first-frame receipt and clean exit. The test checks content pixels, not just window creation. Mesa CPU Vulkan black output was reproduced; the OpenGL preference fixed the default path.
- Native Secret Service credential create/read/update/delete, including Unicode and missing entries, with an isolated test collection.
- Linux: core 72, protocol 16, input 11, client 126, host 71 and codec 5 library tests passed. Native PAM authentication separately passed with a temporary non-root OS account (valid and invalid passwords). macOS workspace strict Clippy passed; Windows GNU cross-check passed including network and credential probes.

## Native CI

The workflow builds Windows x86_64 and Linux x86_64 desktop, daemon and tray,
runs native desktop probes, and packages all three executables with checksums.
The first run built both platforms and exposed a daemon smoke-test setup error;
the corrected workflow is pending verification. Final run URLs and artifact
results will be recorded after completion.

## Boundaries

The virtual desktop exercises the real capture/input APIs and real QUIC loopback
transport. It does not certify physical GPUs, WAN jitter, multi-monitor scaling,
locked/UAC secure desktops or tray behavior in every desktop shell. Linux needs
X11 for hosting and an AppIndicator-capable panel for a visible tray. Wayland
hosting, system-audio capture, pre-login access, native Computer Use and automatic
package installation remain unavailable. Unsupported capabilities are not
advertised as working. Use the account password rather than a Windows Hello PIN.

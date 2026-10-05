# Tray, daemon and desktop lifecycle review — 2026-09-30

Reviewed commit: `3505d57a9dc3efd18bd9a8f6e8afa372178376e5`.

No actionable defects were found in the reviewed macOS process separation,
main-window shutdown, or daemon CLI management paths. No runtime code changes
were needed. Validation used freshly built binaries and private data directories
and launchd labels; sharing was disabled in live daemon fixtures.

## Process boundaries

- `removentd` owns hosting and management IPC. It holds `.daemon.lock` for its
  data directory. The shared Rust `Service` serializes management operations,
  uses a per-user Aqua LaunchAgent, and separates login registration from the
  lifetime of the current process.
- `RemoventTray` is a separate Swift executable with `.tray.lock`,
  `LSUIElement=true` and AppKit's accessory activation policy. Quitting it closes
  its IPC client and terminates only the tray. Its watchdog uses `daemon ensure`;
  a persistent explicit stop takes precedence over automatic recovery.
- `removent` is the GPUI desktop executable. Its `on_window_closed` callback in
  `apps/desktop/src/main.rs` quits when the home window is absent. It does not
  send daemon shutdown or terminate the tray. Closing only a viewer leaves the
  home window present; closing the home also ends this desktop's viewer sessions.
- The tray's `openMainApp` prefers its containing application bundle and passes
  the same `REMOVENT_DATA_DIR`. Development startup uses the matching desktop
  executable. Desktop and tray use the same service manager through Rust/CLI.

## Verification evidence

| Check | Result |
| --- | --- |
| Locked desktop, daemon and CLI build | Passed |
| Release Swift tray build and resource packaging | Passed |
| Core service tests, including opt-in launchd lifecycle | 3 passed |
| Daemon IPC integration tests | 8 passed |
| Swift tray integration suite | Passed: singleton startup, status, PIN, admission and polling |
| Real background lifecycle acceptance script | Passed all five phases |
| Native main-window close | Passed with computer-use clicking the native close button |
| Main application reopen | Passed by reopening the same isolated bundle and data directory |
| Native tray menu click to reopen | Unverified: computer-use could not bind the menu-bar helper |

The real lifecycle script covered tray-independent startup, concurrent tray and
daemon singleton attempts, daemon crash recovery, recovery after removing the
launchd job, explicit stop across tray relaunch and simulated login loading,
IPC shutdown persistence, login toggles without interruption, recovery with both
UIs closed, and the relocated embedded tray surviving desktop termination.

For the native close check, desktop PID `10668` rendered its first frame, then
exited after clicking the close button. The application log recorded:

```text
home window closed; quitting desktop (tray and daemon remain running)
```

Original daemon PID `94852` and tray PID `94884` stayed alive. `daemon
service-status` still returned `managed: true`, `reachable: true`, and
`stopped_by_user: false`. `daemon status` returned a live response with
`tray_connected: true`. The desktop was absent from the subsequent macOS running
application inventory. This confirms desktop termination and removal of its Dock
running state; the Dock itself was not visually inspected. A pinned Dock shortcut
is controlled separately by macOS.

With the desktop closed, real CLI `restart`, `stop`, `service-status`, `ensure`
and `start` all behaved correctly. Restart preserved the disabled sharing
preference and the tray process. `service-status` remained usable while stopped;
`ensure` respected that stop. Test services and tray processes were cleaned up.

## CLI usage

The installed CLI is `/Applications/Removent.app/Contents/MacOS/removent-cli`
(or the corresponding path under `~/Applications`). Source builds use
`target/debug/removent-cli`. Set `REMOVENT_DATA_DIR` when managing a custom root.

| Command suffix | Meaning |
| --- | --- |
| `daemon start / stop / restart` | Manage process lifetime; explicit stop persists |
| `daemon service-status` | Query login registration, launchd ownership, socket reachability and stop intent, including while stopped |
| `daemon status` | Query the live daemon's sharing, listener readiness, sessions, relay and permissions |
| `daemon enable / disable` | Change and persist sharing without terminating management IPC |
| `daemon login-on / login-off` | Manage login startup; login-off leaves the running service intact |
| `daemon ensure` | Automatic recovery that respects explicit stop |
| `daemon permissions` | Request interactive permission setup from the daemon process |

`status.running` is the sharing preference, not proof of daemon process liveness;
use `service-status.reachable` for IPC connectivity and `status.host_ready` for
the actual host listener. Capture permissions, remote session continuity and a
real logout/login cycle were outside this review's live acceptance scope.

# Startup management validation — 2026-10-22

## Implemented

- Settings → General manages user-login startup and administrator-installed
  pre-login access separately, with actual registration state, busy controls,
  errors/recheck, approved-device refresh and system-version update notices.
- The installed desktop applies user-login defaults even when its launcher is
  bypassed. Signed/notarized installations offer administrator setup for the
  LoginWindow agent by default. Explicit disable/stop choices and cancellation
  survive relaunch. Enabled system copies are offered updates with app updates.
- Packaged CLI: `daemon system-status`, `system-enable`, `system-update`,
  `system-disable`. System management does not write logs through a user-owned
  data directory while running as root.
- System installation verifies the signing team and Gatekeeper assessment,
  stages root-owned code/configuration, rejects unsafe paths/symlinks and strips
  setuid/setgid/write privileges. Failed publication restores the previous
  installation. Updating approvals preserves a disabled startup choice.
- The system snapshot retains device identity and approved video/input grants,
  copies relay routing privately, and enforces macOS account authentication.
  App passwords/OTP secrets are not copied; an empty trust list admits no peer.
- LoginWindow shutdown respects the stop marker so KeepAlive cannot undo an
  explicit disable. The existing user-session host retries transient bind errors
  during session transitions.

## Automated checks

| Check | Result |
| --- | --- |
| `cargo test -p removent-app -p removent-cli -p removent-core -p removent-daemon --locked` | 248 passed, 5 ignored |
| `cargo test -p removent-core service::tests::launchd_lifecycle --locked -- --ignored --exact` | 1 passed, isolated GUI launchd job |
| Strict Clippy for the four changed packages, all targets | Passed |
| `cargo fmt --all --check`, `git diff --check`, repository layout | Passed |
| Desktop/CLI/daemon debug build | Passed |

Coverage includes snapshot filtering and private permissions, fresh empty trust,
secret-safe parse failures, unsafe file rejection, transactional update rollback,
disabled-update preservation, privilege-bit removal, explicit choices and
cancelled updates, CLI privilege/code gates, and UI duplicate-action protection
at 860×600 in dark/light themes. Existing Keychain tests also remain green.
The existing `block 0.1.6` future-compatibility warning remains.

## Native runtime checks

Used a separate ad-hoc preview bundle and short `/tmp/removent-startup-*` data
directory. Its `removentd` was a synthetic IPC fixture, with no network listener,
capture, input or user credentials. The actual macOS Service manager handled it.

- Opened General settings in the real desktop, inspected Chinese dark/light and
  English light layouts.
- Toggling login startup on produced a real isolated LaunchAgent registration;
  CLI reported `launch_at_login=true`, `managed=true`, `reachable=true`.
- Toggling off removed future login registration without changing fixture PID
  86428 or its reachable/running state.
- Closing the desktop terminated the desktop process while PID 86428 and IPC
  remained available. Reopening preserved login startup off and the same agent.
- Attempting system setup from the ad-hoc bundle failed before administrator
  elevation, displayed an actionable error and retained the uninstalled state.
  Recheck cleared the resolved error in both the form and status bar.
- Stopped the isolated service and closed the preview after testing. Its login
  entry was absent, processes exited, and final service status was
  `launch_at_login=false`, `managed=false`, `reachable=false`.

The first preview used the long macOS temporary-directory path, which exceeded
the Unix socket path limit. It was stopped and replaced with the short `/tmp`
fixture before lifecycle acceptance; no failed job was left running.

## Explicitly not verified

The user deferred real logout/restart testing. No system LoginWindow service was
installed, no administrator consent was supplied, and the development Mac was
not locked, logged out or restarted. Signed privileged publication, root-process
TCC, actual pre-login video/input/account entry, user-session handoff, relay
reconnection and hardware/macOS-specific behavior remain unverified. FileVault
preboot unlock is a separate flow, not implemented by this change.

The installed `/Applications/Removent.app` was inspected read-only and already
reported user-login startup enabled. It was not replaced; changes remain in the
working tree and are not committed, pushed or released.

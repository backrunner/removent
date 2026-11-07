# Host unattended update audit

Historical audit of 0.1.11, before implementation. The 0.1.12 implementation and acceptance are recorded in [automatic-update-2026-11-07.md](automatic-update-2026-11-07.md).

Date: 2026-11-07. Scope: existing desktop updater and host service lifecycle. No unattended installation policy is enabled by this release, and the existing br-gateway service is preserved.

## Current behavior

- `apps/desktop/src/engine/preferences.rs` checks for updates 30 seconds after desktop startup, then every 24 hours when enabled. Download and install remain separate user actions. Neither the daemon nor the tray runs this scheduler; a headless host cannot discover an update on its own.
- Installation rejects active controller or host sessions at its initial check. This check is not a maintenance lease: it does not prevent a new remote session from arriving before replacement.
- `apps/desktop/src/updater/install.rs` validates the selected channel/version, signed manifest, download hash, Apple signing identity and staged bundle. Replacement uses same-volume rename with an `.app.old` backup.
- The installer stops the owned tray, restarts the daemon through the existing serialized service lifecycle, then starts the replacement tray and desktop. Explicit user stop intent and launch-at-login preferences are preserved.
- If Launch Services rejects the desktop launch, the old bundle and background processes are restored. An accepted launch followed by an early crash has no independent watchdog or automatic rollback. The old bundle is cleaned up after the new desktop's first frame.

## Feasible extension

Unattended update and host restart are feasible, but need a lifecycle coordinator beyond calling the existing install action on a timer:

1. Put update discovery in a process that remains available when the desktop is closed. Keep the existing signature, channel and upgrade-only rules and make automatic installation an explicit persisted policy.
2. Let downloads occur without disrupting sessions. Acquire a host maintenance lease before replacement, atomically reject new session admission, and wait for existing sessions to end. Defer while busy or while another installation is in progress.
3. Run replacement from an independent helper that survives the old process exiting. Preserve device identity, credentials, user stop intent and login preferences; do not start a host that was deliberately stopped.
4. Require a bounded health acknowledgement from the replacement daemon, including its version and functioning control socket. On launch failure, crash or timeout, restore the old bundle and daemon automatically. Record the failed version to avoid repeated restart loops.
5. Treat the user-session LaunchAgent and privileged login-window LaunchDaemon as separate installation contexts. The latter needs an authenticated privileged path and cannot inherit a desktop-only replacement design.

Acceptance should cover desktop-closed operation, busy-session deferral and admission races, manual-stop preservation, channel changes, interrupted downloads, signature rejection, unavailable disk space, launch failures and early crashes, and rollback with the same host identity. First run these against isolated local service labels and data directories, then schedule a separate remote rollout. The current audit is not evidence that unattended host updating already works.

## Existing lifecycle acceptance

The opt-in `packaged_update_restarts_tray_and_daemon_and_recovers_rollback` test uses the real packaged Swift tray, real CLI and launchd jobs, with a Python daemon fixture. Its temporary data directory selects a distinct hashed launchd label. It exercises replacement, explicit rollback, stale-tray refresh, and preservation of manual stop/login intent. It does not exercise a full signed download/install, an actual media host, or independent crash recovery.

Re-run result on 2026-11-07 with the 0.1.11 source: **passed**, one acceptance test in 9.21 seconds. All four lifecycle assertions above passed. Temporary processes and launchd registration were cleaned up by the fixture.

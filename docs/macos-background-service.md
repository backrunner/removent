# macOS installation and unattended hosting

## Installer

The DMG opens in Finder icon view, with a 700 × 450 point background containing
1× and 2× TIFF representations. Double-clicking Removent opens **Install and
Open**: the launcher copies and verifies the signed bundle, installs it in
`/Applications` (or `~/Applications` when the system directory is not writable),
then opens the installed copy. Dragging to Applications remains supported.
The launcher retains the previous version during replacement and rolls back on
failure. Quit the existing main window and menu bar app and stop its daemon before
replacing it. The installer checks the background job as well as both UIs and
refuses to replace running code. Prefer the in-app updater for an existing
installation.

`scripts/make_dmg.sh` checks the Finder view, icon positions, background alias and
both artwork resolutions. Release verification also mounts and checks the final
compressed DMG. Local ad-hoc builds are for testing; distribution still uses the
existing Developer ID signing and notarization pipeline.

## Process ownership

`removentd` owns hosting, device identity, pairing, capture and input. The GPUI
client and Swift menu bar app are independent management clients. Closing either
or both leaves the server running. The existing Swift tray is retained because
it provides pairing PINs, admission prompts, permission setup and service controls
without keeping a desktop window open. It is optional for a configured host.

Packaged launches use a per-user **LaunchAgent** in the graphical `gui/<uid>`
session, with `KeepAlive`, a 10-second restart throttle and graceful SIGTERM
handling. Normal starts bootstrap a transient job from the data directory.
**Start Server at Login** persists the same job in
`~/Library/LaunchAgents/com.removent.daemon.plist`. Turning that option off removes
future login registration without terminating the current service or session.
**Show Menu Bar App at Login** is a separate, optional launch-once job; quitting
the tray does not respawn it or stop the server.

All three entry points use the same Rust service manager. The daemon's data lock
prevents duplicate instances. Custom/development data directories have separate
job labels, so testing cannot manage the installed server accidentally. An old
daemon spawned directly by the app is left running when enabling login startup;
launchd takes ownership after an explicit restart or the next login. Source
builds launched by `scripts/dev.sh` retain that script's process lifecycle.

This extends the existing LaunchAgent implementation without administrator
privileges. A system LaunchDaemon is unsuitable for the current capture pipeline:
it runs outside the user's Aqua session and does not inherit that user's TCC
grants. A future ServiceManagement/SMAppService migration should include signed
upgrade, background-item approval and TCC attribution tests on supported macOS
versions; it is not needed to make the current processes independent.

## First-time unattended setup

1. Install and open Removent. Its server starts independently from saved settings.
2. In the menu bar, choose **Set Up Screen Sharing Permissions…**. This asks from
   the running daemon's own process, which is the identity that needs Screen
   Recording and Accessibility. Grant both in System Settings, then use **Restart
   Background Service**. Check the menu/CLI permission indicators afterwards.
   An app permission grant alone is not evidence that the launchd process has it.
3. Complete PIN pairing with each authorized controller while someone is present.
   Existing pairing stores the certificate and capability grants. Trusted devices
   can reconnect within those grants without an app or tray admission dialog.
   Unknown devices still need pairing; expanded capabilities still need approval.
   `DenyAll` continues to reject incoming sessions. VNC is a separate, optional
   compatibility service and does not use this trust model.
4. Enable **Start Server at Login**. Optionally enable the separate tray login
   item. Check System Settings → General → Login Items if macOS blocks background
   activity. Disable automatic system sleep as appropriate for the host's use.
5. Close the client and tray, reconnect from the paired controller, then test a
   logout/login cycle on the intended host before depending on unattended access.

Automatic/background starts never show TCC consent dialogs. They expose missing
permissions through IPC and logs so startup remains reachable without a person
dismissing a dialog. The sharing switch is saved before success is acknowledged;
disabling sharing survives both crashes and logins.

**Supported boundary:** hosting in a logged-in graphical user session. A restart
followed by a user login starts the server. FileVault preboot unlock, the initial
login window, logged-out users and a sleeping machine are outside the current
ScreenCaptureKit host's support. A locked session and display-less operation need
hardware/macOS-specific validation; do not equate them with a guaranteed remotely
controllable login screen. Background registration cannot grant privacy access,
unlock FileVault or wake a sleeping Mac by itself.

## Server-only operation

The CLI is included inside the installed bundle; no Rust installation is needed.
Replace `/Applications` with `~/Applications` for a per-user installation.

```sh
REMOVENT_CLI=/Applications/Removent.app/Contents/MacOS/removent-cli
"$REMOVENT_CLI" daemon start          # Independent process; respects saved sharing switch
"$REMOVENT_CLI" daemon enable         # Enable sharing and persist it
"$REMOVENT_CLI" daemon permissions    # Interactive first-time privacy setup
"$REMOVENT_CLI" daemon status         # Sharing, sessions and actual daemon TCC grants
"$REMOVENT_CLI" daemon login-on       # Start now and persist future user logins
"$REMOVENT_CLI" daemon service-status # Registration, launchd ownership and IPC reachability
"$REMOVENT_CLI" daemon login-off      # Remove login registration; keep the current server
"$REMOVENT_CLI" daemon disable        # Stop sharing, keep management IPC, persist disabled
"$REMOVENT_CLI" daemon restart        # Restart process, e.g. after granting Screen Recording
"$REMOVENT_CLI" daemon stop           # Stop process now; login preference is unchanged
```

`stop`/`restart` interrupt any active session; the tray disables restart while a
session is reported. The bundle launcher also accepts `--server`, optional
`--login` and `--tray`, e.g.:

```sh
/Applications/Removent.app/Contents/MacOS/removent-launcher --server --login
```

Data stays in `~/Library/Application Support/removent/userdata`; logs are under
`logs/`, and `REMOVENT_DATA_DIR` overrides the directory. The manager pins an
absolute data path in each job. To remove background operation before deleting
the app, turn off the tray login item, run `daemon login-off`, then `daemon stop`.
Deleting the app does not remove device identity or settings.

## Verification

```sh
cargo test --locked -p removent-core -p removent-daemon -p removent-host --lib --test ipc
cargo test --locked -p removent-core service::tests::launchd_lifecycle -- --ignored --nocapture
bash tray/Tests/run_integration_test.sh
```

The opt-in launchd test uses a fake Unix-socket server, temporary data/login
directories and a private job label. It covers idempotent start, login toggles
without process interruption, crash recovery, restart and shutdown. It does not
grant privacy permissions or claim to validate real screen capture after reboot.

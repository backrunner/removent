# macOS startup and unattended access

Open **Settings → General** to manage both startup scopes:

| Setting | Scope | Default |
| --- | --- | --- |
| Remote control before login | The macOS LoginWindow session after startup or logout | Install and enable with administrator authorization |
| Start the background service at login | The signed-in user's desktop, independent of the desktop and tray apps | Enabled |

The installed desktop applies these defaults even when launched directly instead
of through its app launcher. Development builds do not install system services.
Explicit disable choices survive relaunch and updates. Cancelling administrator
authorization leaves setup pending; use **Install and enable** to retry.

The pre-login agent is a global LoginWindow-only LaunchAgent. macOS owns its
session lifetime; the normal per-user agent handles the signed-in desktop.
The host listener retries a temporarily occupied port during the transition.
This is not a root daemon attempting to capture an unrelated WindowServer session.

## Authorization and updates

Pre-login access uses macOS account authentication and permits only controllers
listed under the setting. Connect and trust a controller while signed in, then
choose **Update approved devices**. Video and input grants are copied exactly;
an empty list admits no controller. After revoking a device in the regular trust
store, update this list as well. The UI marks differing authorization as pending.
Closing the app or disabling ordinary sharing does not revoke this separate
administrator-approved service; turn off **Remote control before login** to
disable it.

The installer verifies the Removent Developer ID, signing team and Gatekeeper
assessment before elevation and again after copying. Executables, device
identity, relay routing and approved peers live under the root-owned
`/Library/Application Support/Removent/LoginWindow`. User-writable files are never
consumed live by the privileged host. App-managed passwords and OTP secrets are
not copied. The root host enforces macOS account authentication on every reload.

An app update requests administrator authorization to update an enabled system
service. Cancellation leaves the existing service and an update notice. **Update
approved devices** also updates its binaries. Installation stages and validates
everything before publication; a publication failure restores the old files.
Disabling prevents restart, requests graceful shutdown over private IPC, and
removes future LoginWindow registration. It does not stop the user's live agent.

## CLI

Use the CLI inside a signed, notarized app. Preserve the intended user's data
directory explicitly when using sudo:

```sh
removent-cli daemon system-status --json
sudo env REMOVENT_DATA_DIR="$HOME/Library/Application Support/removent/userdata" \
  /Applications/Removent.app/Contents/MacOS/removent-cli daemon system-enable --json
sudo /Applications/Removent.app/Contents/MacOS/removent-cli daemon system-disable --json
```

`system-enable` installs or updates both the executable copy and approved devices.
`system-update` updates the copy and approvals while preserving whether pre-login
startup is enabled; the settings page uses this for **Update approved devices**.
The existing `login-on`, `login-off` and `service-status` commands still refer only
to the user's session. `login-off` leaves active remote sessions running.

## Acceptance boundary

Registration is not evidence that a particular Mac can capture or control its
login screen. Verify a signed installation on a dedicated Mac: root-process
Screen Recording and Accessibility permissions, direct and relay access after
logout/restart, account authentication, keyboard input, login handoff, revocation,
disable, crash recovery and an app update. Do not log out the active development
workspace as a substitute for this acceptance.

This feature targets the macOS login screen, not FileVault preboot unlock.
Apple documents a separate [SSH FileVault unlock flow](https://support.apple.com/en-ca/guide/deployment/dep82064ec40/web)
for eligible Apple silicon Macs running macOS 26 or later. Removent does not
implement that flow. See [the unlock investigation](macos-unlock.md) for capture
constraints and the dedicated-machine procedure.

# macOS remote unlock investigation (2026-09-20)

Remote-control authentication and macOS account unlock are different operations.
The account password must be entered into the operating system's login UI (or
Apple's supported SSH FileVault flow), after the remote transport is authorized.
Removent must not store account passwords on the host or attempt to bypass TCC,
Secure Enclave, FileVault or the login window.

## What other applications do

| Implementation | Evidence | Consequence for Removent |
| --- | --- | --- |
| RustDesk | Its system `daemon.plist` starts a service; its global `agent.plist` explicitly lists **LoginWindow** and **Aqua** session types. Its Quartz backend uses `CGDisplayStreamCreateWithDispatchQueue`; input is posted to the HID tap. | A per-user Aqua-only LaunchAgent with ScreenCaptureKit is not equivalent to its login-window architecture. A root daemon alone also does not supply a WindowServer capture session. |
| AnyDesk | The macOS installation guide distinguishes full installation (client **and Service**, access while logged out/switching users, unattended access and restart reconnection) from portable installation. | Merely putting an app in Applications or keeping a tray alive does not establish login-window support. Its implementation is proprietary; the guide is behavior evidence, not proof of its capture mechanism. |
| Apple Screen Sharing | OS-owned screen sharing requires explicit sharing authorization and authenticates accounts. Removent already has an ARD type-30/VNC viewer, but that is separate from its own RVP host. | Using Apple's system service is a possible compatibility path; it still needs real-Mac login/locked-session interoperability testing, and the existing synthetic ARD tests do not prove it. |

Sources (read live, not copied into product code):

- [RustDesk agent plist](https://github.com/rustdesk/rustdesk/blob/master/src/platform/privileges_scripts/agent.plist)
- [RustDesk daemon plist](https://github.com/rustdesk/rustdesk/blob/master/src/platform/privileges_scripts/daemon.plist)
- [RustDesk Quartz capture](https://github.com/rustdesk/rustdesk/blob/master/libs/scrap/src/quartz/capturer.rs)
- [RustDesk macOS platform](https://github.com/rustdesk/rustdesk/blob/master/src/platform/macos.rs)
- [AnyDesk macOS installation](https://support.anydesk.com/docs/install-anydesk)
- [Apple Screen Sharing](https://support.apple.com/guide/mac-help/share-the-screen-of-another-mac-mh14066/mac)

## FileVault has a version-specific supported path

Apple's [Intro to FileVault](https://support.apple.com/guide/deployment/intro-to-filevault-dep82064ec40/web)
now explicitly says: **Apple silicon, macOS 26 or later**, Remote Login enabled,
and a network connection permit unlocking FileVault **over SSH after a restart**.
Thus the blanket statement that FileVault can never be remotely unlocked is no
longer correct. This is distinct from injecting a password into a GUI.

The support statement does not establish cold-power-on behavior, Intel support,
older-macOS support, or that Removent's user daemon runs before disk unlock. A
relay cannot reach a host bridge that has not started. Off-LAN SSH access needs
a separately reachable network path (for example an independently running LAN
gateway); an outbound tunnel hosted by the locked Mac's user daemon is not that
path. Retain SSH host-key verification and use Apple's account authentication.
See also [Apple FileVault management](https://support.apple.com/guide/deployment/manage-filevault-with-mdm-dep0a2cb7686/web).

## Implemented paths and validation boundary

Removent currently has an independent per-user daemon, persistent host switch,
trusted-device admission, launchd restart and optional relay reconnection. It
injects input through the HID tap. **This does not yet establish reliable unlock.**
The standard ScreenCaptureKit delegate still reports fatal capture failures. An
opt-in `window_server_capture = true` setting now selects a CGDisplayStream
backend using a dedicated dispatch queue, bounded latest-frame delivery, checked
IOSurface bounds, and up to three stream restart attempts. It offers video/input
only; audio is disabled in negotiation and clipboard bridging is disabled. This
legacy API is deprecated on macOS 14+, so continued availability and actual lock
UI capture must be verified per supported release.

1. **Locked, already logged-in user:** the opt-in backend is implemented; validate capture of the actual lock UI,
   password key events (including non-US layouts), successful unlock, and capture
   recovery on macOS 13/15/26. If SCK cannot capture that UI, implement a supported
   capture/session fallback after validating the chosen API on those systems.
2. **Logged out / initial login window:** an explicit admin installer now
   publishes a global **LoginWindow-only** agent and a signed app copy. Its
   `--login-window` entry point checks root ownership and permissions of code,
   data and every ancestor. It ignores the ordinary data-directory override,
   enforces pinned **previously trusted** peers, and forcibly disables pairing,
   VNC, audio, clipboard and update checks, including after IPC settings reload.
   Admission checks only capabilities the host actually provides; requests for
   unapproved video/input fail immediately instead of waiting for an unavailable
   local prompt. Quick resume rechecks the current administrator-managed grants.
   A request must include video permission because the host always sends video.
   launchd owns session entry/exit; the normal user's independent LaunchAgent
   takes over after login. This handoff and the process's TCC identity require
   real signed-install verification before the feature is considered supported.
3. **FileVault (excluded by the clarified request):** document Apple's supported SSH flow for eligible
   systems, separately from the RVP host. A planned authenticated restart is not
   a general-purpose preboot relay solution.

## Acceptance procedure (a dedicated test Mac)

Complete signed-app privacy setup and device pairing first. With the desktop and
tray closed, test normal RVP and relay connections, lock, reconnect while locked,
enter the account password remotely, and verify the real desktop resumes. Check
wrong passwords, keyboard layout, privacy permission revocation, service crash,
network interruption and sleep/wake. Repeat for logout, fast-user-switch and
restart as separate cases. FileVault testing additionally records hardware,
macOS version, Remote Login setup and the independent SSH network path.

Do not lock, log out or reboot the developer's active workspace to substitute for
this acceptance. No real locked-Mac/password-entry acceptance has been performed
in this change. Lock/login-window code is implemented for explicit validation,
but successful remote unlock is **not yet verified**. FileVault integration is
not implemented and is outside the clarified scope.


## Explicit installation for validation

First pair authorized controllers with the normal host and grant video/input.
Use a Developer-ID signed and notarized build containing this change; the
installer rejects unsigned/ad-hoc/untrusted bundles. While logged in, set
`window_server_capture = true` in the normal host's settings and restart its
background service for the locked-user test.

For the logged-out case, review and run:

```sh
sudo python3 scripts/install_login_window_host.py /Applications/Removent.app \
  "$HOME/Library/Application Support/removent/userdata"
```

The optional final argument selects the RVP port (default 48688). This installs
`/Library/LaunchAgents/com.alkinum.removent.loginwindow.plist` and an isolated
app/data copy under `/Library/Application Support/Removent/LoginWindow`.
It does **not** log out/restart the machine, grant TCC, or change FileVault.
The agent first starts when macOS enters its next LoginWindow session. Confirm
actual root-process Screen Recording and Accessibility grants; ordinary desktop
permissions are not evidence for that process. A signed TCC/MDM setup may be
needed on the target machines.

Installation publishes the complete plist atomically after staging the bundle;
failed publication rolls back the new bundle and preserves an existing agent.

The login-window service is a **separate administrator-authorized trust
snapshot**, so disabling sharing or revoking a peer in the ordinary user's tray
does not disable/revoke this installed service. Administrators must manage that
snapshot explicitly. While the login-window daemon is running, its root-only
IPC can be managed using the installed CLI with
`REMOVENT_DATA_DIR=/Library/Application Support/Removent/LoginWindow/data`:
`daemon status`, `daemon disable`, `daemon enable`, or explicit `daemon permissions`.
Do not use the normal per-user `daemon start/login-on` commands for this service.
To remove future login-window access while logged in, remove the global plist
as administrator; after confirming that no login-window process is active,
remove its isolated installation directory. Before replacing a trust snapshot,
disable/uninstall the old global agent and then repeat the signed installation.
Normal updates deliberately do not overwrite this root-owned copy.

These lifecycle and administrative differences are why this remains an explicit
validation installation, not silently enabled at app startup. A unified signed
ServiceManagement installer and user-facing trust/revocation controls remain
required before presenting login-window access as a finished product feature.

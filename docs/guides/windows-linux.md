# Windows and Linux desktop builds

From 0.2.0, portable archives and SHA-256 files are available in
[GitHub Releases](https://github.com/backrunner/removent/releases).
Windows builds are unsigned and may show a publisher warning. Linux archives
are not distribution-managed packages. Updates require manual replacement.

Extract the complete archive to a writable installation directory. Run `removent`
(`removent.exe` on Windows). The desktop launches the sibling `removentd` and
`removent-tray` executables; keep all three together. Closing the main window
leaves sharing available through the tray. The tray can open the main window,
start or stop the service, toggle sharing, show a pairing invitation, and answer
access requests. Quitting only the tray does not stop the host.
Opening the desktop again restores its existing window for the same data directory.
Access prompts default to rejection and expire after 30 seconds. The main desktop
gets the first opportunity to answer; the tray presents an unanswered request after
eight seconds when that desktop is open. Cancellation, an answer in another window,
or losing the background service dismisses the tray prompt.
After connecting to the background service, the tray restarts it if it crashes.
An explicit service stop persists across tray restarts until you choose Start again.

Windows archives include the Visual C++ runtime DLLs beside the executables;
keep these files together as well. A separate runtime installation is not needed.
Keep the data directory on local NTFS storage, which supports atomic replacement
while other processes read their existing file snapshots.
Windows requires an interactive signed-in desktop. The host cannot control the
UAC secure desktop or the sign-in screen. System-account authentication checks
that the authenticated token belongs to the user running the desktop host.
Use the account password, not a Windows Hello PIN. Allow UDP 48688 through the
firewall on networks where incoming native connections are needed.
Usernames may be local names, `DOMAIN\name`, or a domain UPN. An explicit domain
such as `MicrosoftAccount\email@example.com` is preserved during authentication.

Linux hosting currently requires an X11 session. Wayland hosting fails with an
explicit error because portal-authorized capture/input sessions are not yet
implemented. The controller may run under X11 or Wayland. A desktop shell with
AppIndicator support is needed to display the tray (GNOME may require its
AppIndicator extension). Debian/Ubuntu runtime dependencies include GTK 3,
Ayatana AppIndicator, ALSA, PAM, PipeWire, X11, xkbcommon, and Vulkan/Mesa.
The CI Linux x86_64 archive is built on Ubuntu 24.04 and requires glibc 2.39 or
newer. Build from source for older distributions. Install a CJK font family
(for example, Noto CJK) to display Chinese text when the system has none.
Use the login password of the non-root user running the host for system authentication.
Linux verifies both the password and account validity through the system's `login`
PAM policy. An expired or locked account is not accepted. Authentication does not
switch users, and another user's valid password cannot authorize this desktop.
Remembered connection credentials use Windows Credential Manager with local-machine
persistence (not Windows profile roaming), or the Linux
Secret Service (for example, an unlocked GNOME Keyring). No plaintext fallback
is used when the credential store is unavailable. The remember switch is off initially;
credentials are saved only after a successful connection. Later connections reuse
them for the same verified host and authentication method. Use the connection's edit
form to update or remove them. See [authentication and storage](connection-reference.md#unattended-connection-authentication).
CPU-only Linux desktops prefer
the OpenGL software renderer to avoid black Vulkan frames on older Mesa.

H.264 and AV1 use CPU codecs. New macOS hosts select H.264 when a Windows/Linux
controller advertises that HEVC is unavailable; older macOS releases that always
select HEVC need updating. System audio capture, pre-login access, native
Computer Use, idle-sleep prevention, and automatic package installation are not
implemented on these platforms. Text clipboard sharing is supported. Do not infer feature support
from a successful build; see [the validation report](../validation/windows-linux-2026-11-13.md) for tested scenarios.

Data lives under `%LOCALAPPDATA%/Removent/userdata` on Windows and
`$XDG_DATA_HOME/removent/userdata` (default `~/.local/share/removent/userdata`) on
Linux. `REMOVENT_DATA_DIR` overrides either location. Development builds use the
workspace `userdata` only if that workspace exists on the running machine.
Windows management IPC uses a local owner-only named pipe; Linux uses a private
Unix socket. Pairing keys and settings remain local to that data directory.

Build with `cargo build --locked --release -p removent-app -p removent-daemon
-p removent-tray`. Linux build dependencies are listed in
`scripts/build/linux-deps.sh`. CI archives are development packages, not signed
Windows installers or distribution-managed Linux packages.

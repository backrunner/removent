# Windows and Linux desktop builds

Extract the complete archive to a writable installation directory. Run `removent`
(`removent.exe` on Windows). The desktop launches the sibling `removentd` and
`removent-tray` executables; keep all three together. Closing the main window
leaves sharing available through the tray. The tray can open the main window,
start or stop the service, toggle sharing, show a pairing invitation, and answer
access requests. Quitting only the tray does not stop the host.

Windows requires an interactive signed-in desktop. The host cannot control the
UAC secure desktop or the sign-in screen. System-account authentication checks
that the authenticated token belongs to the user running the desktop host.
Use the account password, not a Windows Hello PIN. Allow UDP 48688 through the
firewall on networks where incoming native connections are needed.

Linux hosting currently requires an X11 session. Wayland hosting fails with an
explicit error because portal-authorized capture/input sessions are not yet
implemented. The controller may run under X11 or Wayland. A desktop shell with
AppIndicator support is needed to display the tray (GNOME may require its
AppIndicator extension). Debian/Ubuntu runtime dependencies include GTK 3,
Ayatana AppIndicator, ALSA, PAM, PipeWire, X11, xkbcommon, and Vulkan/Mesa.
Use the login password of the user running the host for system authentication.

H.264 and AV1 use CPU codecs. New macOS hosts select H.264 when a Windows/Linux
controller advertises that HEVC is unavailable; older macOS releases that always
select HEVC need updating. System audio capture, pre-login access, native
Computer Use, and automatic package installation are not implemented on these
platforms. Text clipboard sharing is supported. Do not infer feature support
from a successful build; see the validation report for tested scenarios.

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

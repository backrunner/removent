# UI refresh and Apple Screen Sharing diagnosis

## Interface

The current appearance uses neutral macOS grays, compact segmented section
navigation, subtle separators, and shallow card shadows. The window backing is
opaque: GPUI's blurred-window mode exposed sharp background text on this macOS
version. Search, the empty-device card, section heading, and local-device area
share a 16-point sidebar gutter. The oversized inset content panel and luminous
outlines have been removed.

- [Current light settings](images/settings-native-light.png)
- [Current dark settings](images/settings-native-dark.png)


The settings page now has five navigable sections: General, Appearance, Security,
Sharing, and System. Each section shows only its relevant controls. Shared styles
use neutral surfaces, clear selected states, raised cards, consistent corner
radii, and aligned input sizing. The home screen, device list,
connection picker, pairing dialogs, status bar, and viewer controls use the same
visual system. Both English and Simplified Chinese are supported.

Inputs explicitly fill their containers; previously the device-name field could
collapse to its minimum width. Navigation and segmented controls wrap in compact
windows, and scrollable panes keep settings and connection actions reachable.

Native window review covers the 1040×700 default and 860×600 minimum.
A drag below the minimum was clamped to exactly 860×600 by macOS. Section
titles, protocol cards, input widths, selection/hover feedback, and connection
actions were checked in actual GPUI windows. A local stalled VNC endpoint was
used to check the eight-second waiting hint, ten-second greeting timeout, and
Escape cancellation with the form preserved.

- [Dark settings](images/settings-dark.png)
- [Connection picker](images/connection-dark.png)
- [Compact light appearance](images/settings-light-compact.png)
- [Waiting state](images/connection-waiting.png)
- [Failure and retry](images/connection-failed.png)
- [RDP at the minimum size](images/connection-rdp-light.png)

## Window and interaction refinement

The main window opens at 1040×700 logical pixels and has an 860×600 minimum.
The viewer retains its independent 480×320 minimum. The sidebar narrows to 228
pixels below a 1000-pixel main-window width. Settings keep their heading and
section navigation above a separately scrolling content pane. Connection forms
keep the heading, current state, and actions visible while fields scroll.

All application buttons share explicit normal, hover, pressed, selected, and
disabled palettes, with native keyboard activation, focus rings, and tooltips.
This also avoids gpui-component 0.5.1's hard-coded red hover text. Selected tabs
and segments retain their identity while hovering; primary labels retain readable
contrast in both appearances.

Connection progress comes from backend milestones: resolving, connecting,
negotiating, pairing, authenticating, and preparing the desktop. The form shows
elapsed time and a delayed-response hint after eight seconds. Cancel aborts the
attempt, preserves fields, stops its timer, and permits editing/reconnecting.
Close aborts and dismisses the form. Failure keeps its error beside the retry
action. Repeated submissions are locked immediately; generation filtering drops
stale progress, PIN, success, and failure events after cancellation. Native
Removent pairing temporarily covers the form so it can be restored on failure
or cancellation. Discovered-device connections show a cancellable activity strip. Spinners use
the app’s bundled loader-circle asset; the library’s default loader asset was
missing. Common VNC timeout, refusal, and authentication errors are localized
and include the destination and failed stage.

## VNC evidence

On 2026-09-08, the target `192.168.1.11:5900` returned `RFB 003.889` and security
methods `[30, 33, 36, 31, 32, 2, 35]`. ICMP replies were approximately 0.5 ms.
The diagnostic below sends no username or password and makes no login attempt:

```sh
python3 scripts/probe_vnc.py 192.168.1.11 --security 30
python3 scripts/probe_vnc.py 192.168.1.11 --security 35
```

Type 30 returned the full public DH challenge (generator 5, 4096-bit key) in about
0.64 seconds. Type 35 produced no challenge within five seconds. The previous
client preferred type 35 when a macOS username was supplied, then waited for a
DH challenge. Its outer ten-second timeout surfaced only `deadline has elapsed`.

The client now prefers type 30 when both methods are advertised. It preserves
password-only VNC authentication when no macOS account name is supplied. Named
timeouts distinguish TCP connection, greeting, security negotiation,
authentication challenge/result, and desktop initialization. The app includes
the destination in connection errors and no longer truncates the entire ARD
handshake to ten seconds.

Regression coverage advertises the actual server's security list, verifies type
30 selection, completes encrypted ARD authentication and a raw frame roundtrip,
and checks that a stalled greeting reports its stage and releases the socket.
All 45 client tests and 33 app tests pass. The seven GPUI connection-dialog
tests cover keyboard focus, validation/retry, credential clearing, cancellation,
obscured-modal focus, duplicate-submit prevention, and footer visibility at the
minimum window size. Lifecycle tests also reject stale progress events.

The live probe confirms the protocol-selection failure. Full authentication and
remote desktop viewing on this host still require the user's remote Mac login.

## Local diagnostics

Settings → System now includes a local diagnostics card and an Open Logs Folder
action. The app and daemon write separate bounded logs with connection attempt,
protocol, destination, progress, duration, and errors. Crash reports include stack
traces. See [Local diagnostics](local-diagnostics.md) for paths, rotation, privacy,
and scoped debug logging.

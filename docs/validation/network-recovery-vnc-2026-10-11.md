# VNC viewport and shared connection recovery — 2026-10-11

## Behavior

- The macOS viewer paints the contained frame in its measured canvas bounds.
  Pointer mapping uses those same bounds, including the actual title-bar offset.
  The image no longer expands its flex container below the window.
- Removent direct/relay, VNC and RDP use the same recovery supervisor on desktop
  and mobile. Apple OS route state is sampled every 250 ms using local IPv4/IPv6
  reachability flags; it does not depend on a public ping or DNS service.
- An OS offline state cancels the current transport and shows a network overlay,
  retaining the last picture. Waiting offline does not consume retries. When the
  OS reports online, reconnection starts automatically.
- Other interruptions recover silently in the original viewer. Retry delays are
  0, 1, 2, 4 and 8 seconds, with a 12-second connection-attempt timeout and a
  45-second total online budget. A stable connection of at least 10 seconds resets
  the budget; repeated short handshakes cannot create an infinite retry loop.
- Intentional local/session close does not reconnect. An initial authentication
  failure is reported immediately. Native recovery pins the authenticated host,
  retains the rotating resume token and resolves a one-time invitation only once.
- Input and audio are paused during recovery; held desktop inputs are cleared
  when the transport changes. Generation checks reject stale session events.
- A VNC TCP FIN is an interruption, not successful session completion: RFB has no
  intentional server-end message. This fixes an EOF branch that bypassed recovery.

## Automated evidence

- `rtk cargo test -p removent-client -p removent-mobile -p removent-app -- --test-threads=1`:
  208 passed, 3 explicitly ignored, across 9 suites. Includes shared recovery
  deadlines/offline waiting/rapid-drop limits, a real RFB FIN followed by a new
  connection feeding the original viewer receiver, cancellation preventing a
  third connection, existing RDP interoperability and native session/resume tests.
- `rtk cargo test -p removent-app --bin removent ui::viewer::tests -- --test-threads=1`:
  6 passed after the input-epoch refinement. Canvas bounds and clicks are checked
  at 1280×720, 480×320, 1000×700 and 640×900 window sizes.
- `rtk cargo clippy -p removent-client -p removent-mobile -p removent-app --all-targets -- -D warnings`:
  passed. Cargo reports existing future-incompatibility notices in dependencies.
- `rtk cargo fmt --all --check`, `rtk git diff --check` and
  `rtk proxy python3 scripts/check_layout.py`: passed.
- iOS simulator `RemoventMobileTests`: 19 passed, 0 failed. Covers preserving
  ready/canvas state during silent recovery, the offline overlay, clearing it on
  ready, the final error and rejection of stale recovery events.
  Result: `~/Library/Developer/Xcode/DerivedData/RemoventMobile-admbqooljyjrjuauokhtwevmeybd/Logs/Test/Test-RemoventMobile-2026.10.11_19-50-01-+0800.xcresult`.

The discovery duplicate-alias tests now use explicit sequential observation
instants instead of relying on the clock to distinguish adjacent fixture rows.
No discovery production behavior changed.

## Native macOS checks

An isolated development bundle, `Removent Recovery Review.app`, used a temporary
profile and loopback RFB server. It did not replace the installed app or alter the
machine network configuration. Native screenshots and mouse actions were observed
through the computer-use tool.

- A 1280×720 fixture in a 2560×1440 Retina window shows all four green borders,
  including the bottom edge, with the expected contain sidebars.
- Clicks at window screenshot points (305,205), (1280,754) and (2257,1304) arrived
  at remote points (128,71), (640,360) and (1152,648). Expected targets were
  (128,72), (640,360) and (1152,648): at most one remote pixel of rounding.
- OS-offline injection was scoped to this process by interposing the native
  reachability query. Both the initial connection screen and a live viewer showed
  “网络已断开” and “网络恢复后将自动尝试重新连接。” Removing the injection
  established/restored the session automatically and removed the overlay.
- At 11:58:02 UTC, a server FIN was followed by a new ready session within 4 ms.
  The original window and picture remained visible without an error overlay;
  the fixture connection counter increased from 15 to 16.
- At 11:58:12 UTC, the fixture stopped accepting usable sessions. The early
  retry screenshot retained the picture without an error. At 11:58:27 UTC,
  recovery exhausted the remaining retry budget and the same native window
  displayed “连接已断开” with the retry-limit/timeout and last transport error.
  The close button returned to the connection list.
- Cancelling from the offline overlay returned to the list and prevented
  reconnection after restoring online state (fixture count remained 22).
- The final build includes and visibly renders the offline icon. The review app
  and fixture were stopped afterward; both loopback listeners were closed.

Local fixture evidence is under `/tmp/removent-recovery-review/`: `fixture.py`,
`fixture.json`, the reachability interposer, and `data/logs/removent.log`.

These checks establish local desktop presentation, VNC wire behavior and shared
recovery logic. Physical network unplug/Wi-Fi switching, real iOS devices, WAN,
public relays and live third-party RDP/ARD servers were not exercised in this run.
Non-Apple library consumers keep bounded retries without OS-offline gating.

The final isolated review binary SHA-256 is `41669a9e62a97d877762a3b31b5cf41bd631ccd4c342482a62e3462c207d689d`.
Concurrent sidebar/localization edits appeared after this validation; they were
preserved and are outside the tested change described here.

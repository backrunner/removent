# Native input validation (2026-10-24)

## Fixes

- The host previously created synthetic mouse events without a click count.
  AppKit needs `mouseEventClickState` to distinguish single, double and triple
  clicks. Each session now tracks the button, position and down time, uses the
  host's `NSEvent.doubleClickInterval`, and assigns matching down/up event numbers.
  A drag outside the four-point tolerance cancels the next click's continuation.
- Mouse and scroll events use the session's modifier state. Modifier changes
  have the native `FlagsChanged` event type. Repeated key downs carry the native
  autorepeat flag, which resets on key up.
- Pointer motion and scrolling have a separate rate budget from clicks and key
  downs. Transition overload ends the session and releases held input instead
  of silently losing a transition. Releases still bypass throttling.
- Scroll injection writes fractional deltas into fixed-point fields; point-delta
  fields are integers. The desktop sends the pointer position before scrolling,
  and ends scrolling on focus loss or when the gesture ends outside the picture.
  Host teardown also ends an active scroll gesture.

The existing wire format is unchanged. Click timing is reconstructed on arrival
at the host, so severe network stalls that change inter-click timing remain a
limitation; this is not a WAN jitter acceptance result.

## Evidence

- Input unit tests: 18 passed, including all three buttons, double/triple clicks,
  drag/travel/timeout resets, session isolation, modifier flags, repeat and scroll.
- Desktop binary tests: 129 passed, 4 ignored, including viewer click-pair and
  scroll/focus regressions.
- Client library tests: 110 passed, 2 ignored, including ordered input buffering.
- Protocol tests: 14 passed. No protocol-version change is required.
- New host QUIC tests: 2 passed, covering clicks/typing after a motion flood and
  abrupt transport-loss cleanup of scrolling, three buttons and Command.
- Complete host library tests: 70 passed, 2 ignored with one test thread.
  Earlier runs hit timeouts in the existing VNC handshake and audio/video EOF
  tests; both passed separately, followed by the complete passing run.
- Opt-in native acceptance: passed on this unlocked Mac with Accessibility
  permission. A temporary AppKit window received real HID events through the
  native QUIC control pump and `RealInputSink`: left down/up click counts
  `[1, 2]`, Shift-right-click, middle drag, text down/repeat/up and scroll phases.
  The fixture restores the previously active application before assertions.
- Clippy for input, host and desktop, all targets with `-D warnings`: passed.
  Cargo still reports the existing dependency future-compatibility notice.

Run the GUI acceptance explicitly; it briefly activates a test window:

```sh
rtk cargo test --locked -p removent-host --lib native_quic_events_are_recognized_by_appkit -- --ignored --nocapture
```

These are local source/runtime results. This change has not been installed into
the user's application or daemon, published, or validated on a second physical
machine. Mobile pointer/key translation was inspected; iPhone/iPad physical
touch, external-keyboard gestures and RDP interoperability were not exercised.

References: [Apple click state](https://developer.apple.com/documentation/coregraphics/cgeventfield/mouseeventclickstate),
[Apple double-click interval](https://developer.apple.com/documentation/appkit/nsevent/doubleclickinterval).

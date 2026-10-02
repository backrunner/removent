# Mobile full-screen session and direct English input — 2026-10-02

Connected sessions render the desktop behind floating navigation and key controls.
Landscape uses aspect fill without stretching; three-finger pan can reach cropped
edges. Controls fade after four idle seconds and return through a picture tap or
the corner button. Keyboard use and open session actions keep controls visible;
VoiceOver disables the idle fade and Reduce Motion removes the fade animation.

The landscape viewport follows the current UIKit window size through keyboard
presentation and dismissal, including UIKit's 20-point home-indicator inset
change. Rotation and real window resizing update that size. Manual
zoom and picture position survive both transitions. Portrait fits above the
keyboard. The controls respect the display's safe insets.

The session uses a custom `UIInputView` with English letters, ASCII numbers and
symbols, Shift, Backspace, Space, Return and dismissal. Its receiver implements
`UIKeyInput` without a local text buffer or `UITextInput` composition support.
Each printable ASCII character goes directly to the existing controller input
path; non-ASCII input is discarded. Hardware keys use US HID positions and keep
remote shortcuts and special keys. Credential fields and the clipboard editor
retain their system keyboards.

## Evidence

- Twelve Swift unit tests passed on iPhone and iPad simulators, covering immediate
  character delivery, non-ASCII rejection, Return/Tab/Backspace, modifier and
  hardware-key mapping, aspect fill, coordinate mapping and controller state.
- The RVP UI acceptance test passed on both simulators with the real Rust
  loopback fixture, with no skips. The fixture's `typed_text` records Unicode
  key-down events, so the test verifies that the first `t` reaches the host
  immediately and that the complete input is `test`.
- UI assertions check the full landscape canvas (iPhone 874 × 402 points;
  iPad 1210 × 834 points), idle hiding and recovery, a correctly bounded keyboard
  button, letters/numbers/Backspace, keyboard dismissal, reconnect after
  backgrounding, and unchanged canvas and zoomed image frames when opening and
  closing the keyboard.
- Final results: `/tmp/removent-session-iphone-delivery.xcresult` and
  `/tmp/removent-session-ipad-delivery.xcresult`. The iPad unit run is in
  `/tmp/removent-session-ipad-final.xcresult`.
- Inspected the final screenshots after switching the keyboard's numeric page
  back to letters. All titles remain visible. Local captures are under
  `apps/mobile/DerivedDataSessionPhone/Review/iphone-keyboard-landscape.png` and
  `apps/mobile/DerivedDataSession/Review/ipad-keyboard-landscape.png`, alongside
  portrait and landscape session captures.
- The signed device Debug build succeeded; log:
  `/tmp/removent-session-device-delivery.log`. The connected physical iPhone was
  unavailable, so this change was not installed or visually tested on it.

The fixture does not capture a real desktop or inject physical input. These
checks do not establish physical-device, external-hardware-keyboard, WAN or
VNC/RDP acceptance. Existing pairing-transition invalid-frame and QoS warnings
remain in the UI test logs; the assertions above all passed.

## Follow-up bug review

- Fixed soft-keyboard Ctrl/Option/Command shortcuts losing Shift for uppercase
  letters and shifted punctuation. Shortcut mapping now preserves the physical
  key and implicit Shift; sticky session modifiers also apply to hardware keys.
- Replaced the cached landscape viewport with the current window bounds and
  converted keyboard notifications into that window's coordinates. This retains
  the full picture through keyboard transitions while following rotation and
  real window resizing.
- Reproduced an unusable session menu when opened over the keyboard. Suspend
  remote-keyboard focus while a menu or sheet is presented so it cannot take
  focus back from the menu. Disconnect now dismisses all session sheets and
  actions and clears keyboard/modifier state.
- Restoring controls responds to VoiceOver changes, and the idle task checks
  current presentation state again before fading. Floating header buttons use
  44-point targets.
- Reproduced clipped keyboard titles and unreachable shortcut controls at the
  largest accessibility text size. Keyboard labels now use a fixed font fitted
  to their rows, and connected session chrome caps Dynamic Type at XXXL. The
  connection forms, clipboard editor and gesture sheet retain the user's full
  Dynamic Type size.
- Replaced the system confirmation dialog with a scrollable native actions
  sheet because its lower items remained outside the landscape screen at large
  text sizes. The actions sheet retains full Dynamic Type and defers subsequent
  clipboard/gesture presentation until dismissal; pending actions cannot
  reopen a sheet after disconnect.
- Fifteen unit tests passed on iPhone, including all printable ASCII shortcut
  mappings, invalid viewport values, current-window geometry, and keyboard page
  changes retaining their visible titles and direct input.
- Both extended iPhone RVP integration tests passed with no skips in
  `/tmp/removent-session-review-iphone-final.xcresult`, at default and maximum
  accessibility text sizes. They verify the actual
  host's key code/modifier/down-up sequence for Ctrl+Q and Ctrl+?, keyboard-open
  rotation, closing actions restoring the desired English keyboard, scrolling
  the actions list beyond the idle timeout, opening the gesture sheet after
  dismissal, and disconnecting while that sheet is open followed by reconnect.
- Inspected the final maximum-text-size iPhone keyboard and actions screenshots.
  Labels fit their keys, shortcut controls remain above the keyboard, and the
  final actions are reachable by scrolling. Attachments were exported to
  `/tmp/removent-session-review-phone-final-shots`.
- Final signed device Debug build succeeded in
  `/tmp/removent-session-review-device-actions.log`; physical-device and external
  hardware-keyboard acceptance remain unverified.
- The same fifteen unit tests and both extended RVP integration tests passed
  on iPad with no skips or failures in
  `/tmp/removent-session-review-ipad-final.xcresult`. Final screenshot attachments
  were exported to `/tmp/removent-session-review-ipad-final-shots`.
- Inspected the final iPad maximum-text-size keyboard and actions captures as
  well. Updated review captures are under `apps/mobile/DerivedDataSession/Review`
  and `apps/mobile/DerivedDataSessionPhone/Review`, with `-maximum-type` filenames
  for the accessibility text-size cases.

## Standalone commit review

Earlier review runs above used the working tree, including other pending mobile
and authentication changes. Before submission, the ten session-related files
were isolated against `origin/main` (`3505d57`) and reviewed and built separately.
A private session-sheet width modifier removes a dependency on the pending home
UI changes. Fresh UI-test storage prevents an old fixture certificate pin from
interfering with a new fixture run; this option exists only in Debug builds.

- All thirteen standalone Swift unit tests and both RVP UI tests passed on each
  simulator, with zero failures and zero skips. Final result bundles are
  `/tmp/removent-submit-phone-verified.xcresult` and
  `/tmp/removent-submit-pad-verified.xcresult` (fifteen tests each).
- Both UI scenarios use the real loopback Rust fixture at default and maximum
  accessibility text sizes. The test now restores controls through the corner
  button if they fade during the keyboard-dismissal wait, and waits for the
  keyboard before rotating. An earlier iPad run exposed that test timing issue.
- Inspected final keyboard and actions screenshots on both simulators. Exports
  are in `/tmp/removent-submit-phone-verified-shots` and
  `/tmp/removent-submit-pad-verified-shots`.
- The isolated `scripts/check.sh` passed: layout checks plus thirty-one Python
  tests, with one existing platform-specific skip. Rust fixture compilation and
  its formatting check passed. The signed device Debug build passed in
  `/tmp/removent-submit-device-isolated.log`.

Physical-device, external-keyboard, WAN and VNC/RDP acceptance remain outside
these results. The existing runtime warnings described above remain recorded;
no failing assertion or new unresolved session regression remained in this review.

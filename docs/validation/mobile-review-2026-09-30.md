# Mobile review — 2026-09-30

Scope: the SwiftUI iPhone/iPad app in `apps/mobile/`, its UIKit canvas and keyboard,
Bonjour discovery, audio lifecycle, and the Rust boundary in `packages/mobile/`.
Existing unrelated desktop, updater, vendor, and release changes were preserved.

## Findings addressed

| Priority | Trigger and previous behavior | Change |
| --- | --- | --- |
| P1 | Rename a saved VNC/RDP/relay connection without re-entering its password: saving an empty form field removed the stored credential. | Preserve Keychain credentials by default. An explicit switch replaces or removes them. Rust checks the original protocol, host, port, account, domain, and relay route before reusing a secret. |
| P2 | Connect from a form: a fixed 350 ms delay guessed when the sheet was dismissed; invalid connection-only forms could disappear before showing the error. | Validate without starting a session, then connect from the sheet's `onDismiss`. Invalid fields remain editable. |
| P2 | Delete during Chinese/other IME composition: backspace deleted text on the remote computer. Software Return was forwarded as text. | Keep composition deletion local; send Return/Tab as key events and honor the latched modifiers. |
| P2 | Turn off clipboard: Refresh picture disappeared even though native video refresh was available. | Report refresh capability independently through the Rust/Swift boundary. |
| P2 | Change discovery settings and dismiss with a swipe: scanning retained the previous configuration. | Publish persisted settings and restart discovery when the preference changes; ignore results for disabled protocols. |
| P2 | Clipboard send/copy failed: the sheet still closed; a missing remote value could clear the local clipboard. | Keep the sheet and error visible on failure, and only update the local clipboard when text exists. Opening a sheet releases the remote keyboard focus. |
| P2 | Input failure/background/retry: stale connection text, frame counters, and input state could remain. | Explicit connecting state, consistent disconnect status, reset counters/modifiers/drag state, and retain the canvas subscription for reconnect. |
| P2 | Light appearance rendered the session title/status dark against the black desktop canvas. | Use a dark appearance for the fullscreen viewer and its controls; the connection screens continue to follow the system. |
| P2 | Audio-session activation/deactivation synchronously waited for the system on the main thread. | Serialize audio-session operations on a separate queue, with an epoch check to prevent late activation from starting obsolete playback. |

Other boundary improvements: trim host/port input; reject empty hosts, out-of-range
ports and missing RDP usernames; accept only six ASCII digits for pairing; replace
forced casts on session/list responses with errors.

## Interface changes

- Standard navigation bars, sheets, forms and semantic colors; native Liquid Glass
  primary buttons and a single glass surface for session controls on iOS 26+.
- Group connection/account/relay fields; pin the primary Connect action above the
  keyboard; use standard close/save actions with accessible names.
- Align computer rows, show endpoint ports, use restrained protocol badges and a
  direct empty-state action; constrain wide tablet content.
- Give custom session keys at least 44-point targets, readable VoiceOver names and
  selected states that do not rely only on color. Keep Keyboard outside the
  horizontally scrolling key strip.
- Adapt the protocol picker and row layout to accessibility text sizes; keep
  connection and pairing content scrollable; add a gesture reference.

These choices follow Apple's guidance to use Liquid Glass in navigation and
controls, leaving content on ordinary surfaces:
[Materials](https://developer.apple.com/design/human-interface-guidelines/materials),
[Adopting Liquid Glass](https://developer.apple.com/documentation/technologyoverviews/adopting-liquid-glass),
[Accessibility](https://developer.apple.com/design/human-interface-guidelines/accessibility).

## Validation

Environment: Xcode 27.0 (27A266a), iOS/iPadOS Simulator 27.0 (24A434), arm64.
The current deployment target is iOS 26; earlier-system UI branches were removed
after this review at the user's request.

Local XCTest bundles: `/tmp/removent-mobile-review-iphone-verified.xcresult`,
`/tmp/removent-mobile-review-ipad-final.xcresult`. These have machine-readable
passing summaries; additional focused runs cover the final toolbar and viewer contrast changes.

- Rust mobile suite: 7 passed, including cancellation/generation isolation,
  credential scope, side-effect-free form validation, input bounds, and ABI buffer ownership.
- iPhone 17 Pro: 7 Swift unit tests and 5 UI tests passed with both local fixtures
  running; no skips. Covers Keychain rename/clear/host-change behavior, IME input,
  coordinates, invalid form handling, cancellation, layout captures, RVP and VNC.
- iPad Pro 11-inch M5, dark appearance: 7 unit tests and 2 protocol UI tests passed;
  the three form/layout UI tests also passed against the final build in
  `/tmp/removent-mobile-review-ipad-layout.xcresult`.
- RVP acceptance receives real encoded/decoded fixture frames, delivers touch and
  software keyboard input to the recorder, backgrounds the app, reconnects, and
  confirms frames resume. VNC acceptance exchanges real RFB messages with the
  loopback synthetic fixture and verifies frames/input.
- Inspected light/dark, English/Chinese, maximum accessibility text and tablet
  captures. Screenshots are synthetic test content, not a captured user desktop.

## Limits and follow-up validation

- No physical-device acceptance, WAN/relay reliability, live RDP server acceptance,
  or audio latency/quality measurements were performed. These tests do not establish
  those properties. External hardware keyboard, pointer and VoiceOver interaction
  still need device testing.
- Only the installed iOS 27 runtime was exercised; no iOS 26 simulator runtime is
  installed on this machine.
- Xcode runtime diagnostics reported an invalid-frame warning while bringing up
  the keyboard and a priority-inversion warning during RVP playback. Functional
  tests pass, but these warnings are not treated as proof of a clean performance
  profile. There are also pre-existing vendor unused-import warnings.
- Earlier UI failures were traced to tests editing a right-aligned port field at
  its leading caret and tapping a moving button during keyboard dismissal. The
  tests now select the end of the port and explicitly dismiss the keyboard.
- The initial Xcode diagnostic collector stalled after tests completed; final
  runs use `-collect-test-diagnostics never` and preserve XCTest results/screenshots.

Unmodified screenshots are retained locally in `apps/mobile/DerivedDataReview/Review/`
(ignored build output), including `iphone-connection-zh.png`,
`iphone-session-landscape.png`, and `ipad-connection-landscape.png`.

Reproduction and opt-in fixture commands are in [`apps/mobile/README.md`](../../apps/mobile/README.md).

# Mobile interface refresh — 2026-09-30

The iPhone screenshot exposed a full-screen `ContentUnavailableView` expanding
inside a grouped list. Its primary action occupied a separate row, creating a
large empty card. The replacement sizes to its content and keeps the nearby
computers section visible at the default text size.

## Interface changes

- Home uses an inline “Removent” navigation title, a compact welcome row,
  one prominent action, and a short nearby-discovery hint.
- Home overrides the grouped list's top content margin to 12 points, bringing
  the welcome card closer to the navigation bar without changing card padding.
- Saved and nearby computers use the same aligned symbol and two-line labels.
  Protocol and endpoint share a secondary line. Direct-connect rows no longer
  show a disclosure chevron or a separate protocol badge.
- Connection, account and relay inputs share visible labels, leading alignment,
  spacing and keyboard focus. The bottom Connect action stays above the keyboard.
- Settings use semantic symbols, including iCloud, and fold the long certificate
  fingerprint behind a disclosure. The app version is available in settings.
- Pairing and reconnect actions share the primary button treatment. Session keys
  retain their 44-point targets within one compact glass surface.
- Phone content uses its available width; wide iPad content is constrained to a
  720-point reading column. System colors, text styles and native controls adapt
  to appearance and Dynamic Type.

The design follows Apple's [Layout](https://developer.apple.com/design/human-interface-guidelines/layout),
[Lists and tables](https://developer.apple.com/design/human-interface-guidelines/lists-and-tables),
[Buttons](https://developer.apple.com/design/human-interface-guidelines/buttons) and
[Materials](https://developer.apple.com/design/human-interface-guidelines/materials)
guidance. Native glass is used for controls and navigation; content stays on
ordinary system surfaces. This is an implementation and visual review, not a
claim of complete HIG or VoiceOver conformance.

## Validation

- Simulator build and signed device Debug build succeeded with Xcode 27.
- Eight Swift unit tests passed, including credentials, validation, IME,
  modifiers, coordinate mapping and controller state. Three UI tests for
  cancellation, invalid ports and iCloud configuration also passed in
  `/tmp/removent-mobile-ui-refresh-iphone-final.xcresult`.
- The layout test initially encountered old test bookmarks and an inherited
  SwiftUI accessibility identifier. Fresh UI-test storage and cell-based
  querying resolve these test-isolation issues without changing user storage.
- The corrected layout, RVP and VNC UI tests all passed with no skips in
  `/tmp/removent-mobile-ui-refresh-iphone-layout.xcresult`. RVP includes pairing,
  decoded frames, keyboard/input delivery and reconnect after backgrounding.
- iPhone dark and iPad dark layout tests passed in
  `/tmp/removent-mobile-ui-refresh-iphone-dark.xcresult` and
  `/tmp/removent-mobile-ui-refresh-ipad-dark.xcresult`.
- Inspected unmodified screenshots for Chinese/English, light/dark, landscape,
  maximum accessibility text and the connected session. Local exports are in
  `apps/mobile/DerivedDataUIReview/Review/`, including `home-iphone-dark-zh.png`,
  `connection-iphone-dark-zh.png` and `settings-iphone-dark-zh.png`.
- Installed Removent **0.1.3 (2)** on the connected iPhone 17 and read back that
  version from the device. Signature verification confirmed `get-task-allow`,
  the Removent CloudKit container and the Development environment. The phone
  was locked when automatic launch was attempted.
- The follow-up branding change restores the inline home title to **Removent**
  in every language. Debug build **0.1.3 (3)** built successfully, passed
  signature verification, and was installed with its version read back from
  the iPhone. Automatic launch was again blocked by the locked device. The
  layout screenshots above predate this title-only change.
- The top-spacing follow-up passed the existing layout and Dynamic Type UI test
  with no runtime warnings in `/tmp/removent-mobile-home-gap.xcresult`. Inspected
  the Chinese home in light and dark appearance; updated captures are under
  `apps/mobile/DerivedDataUIReview/Review/home-gap/` and
  `apps/mobile/DerivedDataUIReview/Review/home-gap-dark-zh.png`.
- Debug **0.1.3 (4)** built, passed signature verification, and installed on the
  iPhone. After a temporary device disconnect, the installed version was read
  back successfully. Automatic launch was blocked by the locked device.

The simulator tests do not establish physical-device session performance or WAN,
RDP, audio-latency and VoiceOver acceptance. RVP testing still reports the prior
QoS warning; some keyboard-transition runs report a SwiftUI invalid-frame warning.
The final iPhone/iPad dark layout runs reported no runtime warnings.

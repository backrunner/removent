# Removent Mobile

The iPhone and iPad app is a controller only. The Mac remains the controlled
host; the mobile app shares the Rust controller session engine with Removent
desktop and presents it through a small SwiftUI shell.

The controller supports native Removent/RVP, VNC / Apple Remote Desktop, and
RDP connection forms. Native sessions follow the desktop host authentication policy: pairing code (default),
access password, authenticator OTP, or no authentication. Policy and OTP setup live in
desktop Settings → Security; the mobile app prompts only for the required credential.
Desktop Security settings also choose whether paired controllers must pair each connection.
Enter a host-generated 12-digit connection code in the address field to connect by Bonjour on
LAN, or select the same relay and use that code instead of the room. No host fingerprint is
needed before invitation pairing. After success, **Remember this computer** saves the resolved
destination and verified fingerprint, never the temporary code.
Native sessions include mutual certificate
pinning, relay routing, adaptive video receive, touch/mouse and keyboard input,
text clipboard, optional Opus playback, saved connections in Keychain, Bonjour
discovery, one quick resume, and explicit foreground/background session state.

Requires iOS/iPadOS 26 or newer and Xcode 26 or newer (validated with Xcode 27).
Controls use native Liquid Glass directly. The connection forms support Dynamic
Type and iPad sheets; the
session's floating keyboard controls use one glass surface with 44-point targets.
Session chrome caps text scaling at XXXL so controls stay above the landscape
keyboard; forms and sheets retain full Dynamic Type, and key labels fit their rows.
Connected sessions show the desktop behind floating controls, which fade after
four idle seconds. Tap the picture or the corner button to reveal them; controls
stay visible while typing or using a session action. Landscape fills the display
without stretching (edges can be cropped; use three-finger pan to reach them).
Opening the keyboard in landscape overlays the picture without resizing or
resetting its zoom. Portrait continues to fit above the keyboard.
The session keyboard has a fixed English layout with ASCII numbers and symbols,
no language switch, dictation or candidate area. Every character goes directly
to the computer. Hardware keyboards use US key positions and retain remote
shortcuts, arrows and other special keys. Connection credentials and the
clipboard editor keep their normal system keyboards.
See the [full-screen and direct-input validation](../../docs/validation/mobile-session-2026-10-02.md)
for simulator evidence and remaining device checks.
Editing a saved connection keeps its Keychain password unless explicitly
replaced. Changing the computer or account requires entering the password again.

Generate or refresh the Xcode project after changing `project.yml`:

```sh
xcodegen generate --spec apps/mobile/project.yml
```

Build the Rust static library for a simulator or a device:

```sh
scripts/build/build_mobile.sh --platform iphonesimulator --configuration Debug
scripts/build/build_mobile.sh --platform iphoneos --configuration Release
```

Then open `apps/mobile/RemoventMobile.xcodeproj` in Xcode. The Rust library is
generated into `apps/mobile/Libraries/` and is intentionally ignored by Git; a
clean checkout rebuilds it through the Xcode pre-build script.

The synthetic loopback host used by the RVP UI test is opt-in:

```sh
cargo run -p removent-host --example mobile_fixture
python3 scripts/fixtures/mobile_vnc_fixture.py
xcodebuild -project apps/mobile/RemoventMobile.xcodeproj -scheme RemoventMobile \
  -destination 'platform=iOS Simulator,name=Removent-Mobile-Review' \
  -parallel-testing-enabled NO -collect-test-diagnostics never test
```

The RVP acceptance test proves the mobile app receives real decoded frames and
that touch/keyboard input reaches the fixture. It does not prove performance on
a physical device or WAN quality. The VNC/RDP client engines remain the same
Rust implementations exercised by the desktop interop suites; a live RDP
server is required for mobile RDP acceptance. Run each fixture in a separate
terminal. By default both listen only on loopback; `REMOVENT_FIXTURE_INVITATION=1` makes the
RVP fixture advertise a one-time code over Bonjour and listen on IPv4 interfaces. The VNC fixture exercises raw RFB frames
and records input without controlling the computer. The RVP test additionally
types through the software keyboard and reconnects after backgrounding.

The suite also captures portrait, landscape, Chinese, and maximum Dynamic Type
layouts. Repeat on an iPad simulator with dark appearance for tablet coverage.
The review and remaining validation limits are in
[`docs/validation/mobile-review-2026-09-30.md`](../../docs/validation/mobile-review-2026-09-30.md).

The app requests Local Network access for Bonjour discovery and uses the mobile
Keychain access group declared in `RemoventMobile.entitlements`. It does not
store passwords in `connections.json`.

Saved connections can sync with the macOS app through the same user's private
CloudKit database after both builds are provisioned for
`iCloud.com.alkinum.removent`. Enable Settings → iCloud → Sync saved connections.
Passwords and pairing remain local; imported credentialed connections open their
form for review. Builds without CloudKit provisioning show an explicit configuration
status and retain local functionality. Setup, conflict handling and remaining live
acceptance are documented in [`docs/architecture/cloudkit-sync-design.md`](../../docs/architecture/cloudkit-sync-design.md).

The Rust build script currently produces arm64 libraries for physical devices and
Apple Silicon simulators. The Xcode project declares that supported architecture.

The synthetic RVP fixture also accepts `REMOVENT_FIXTURE_AUTH=password|otp|none`.
Password is `fixture-password`; OTP setup secret is `GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ`
(TOTP, SHA-1, 6 digits, 30 seconds). These are synthetic loopback-only test credentials.

Relay setup uses the relay address and optional SNI (empty uses the address
hostname). The form does not expose certificate fingerprint fields. **Relay access
password** is optional and supplied by the relay administrator when access is
restricted. The first connection asks the user to trust the computer, and an
unknown QUIC relay is confirmed separately before any relay access password is
sent. Cancelling stops the connection. Successful connections remember the exact
certificate locally; subsequent connections check it and reject changes.
Certificate verification exceptions remain local when connections sync.

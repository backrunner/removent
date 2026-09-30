# Removent Mobile

The iPhone and iPad app is a controller only. The Mac remains the controlled
host; the mobile app shares the Rust controller session engine with Removent
desktop and presents it through a small SwiftUI shell.

The controller supports native Removent/RVP, VNC / Apple Remote Desktop, and
RDP connection forms. Native sessions include PIN pairing, mutual certificate
pinning, relay routing, adaptive video receive, touch/mouse and keyboard input,
text clipboard, optional Opus playback, saved connections in Keychain, Bonjour
discovery, one quick resume, and explicit foreground/background session state.

Requires iOS/iPadOS 26 or newer and Xcode 26 or newer (validated with Xcode 27).
Controls use native Liquid Glass directly. The connection forms support Dynamic
Type and iPad sheets; the
session's floating keyboard controls use one glass surface with 44-point targets.
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
terminal; both listen only on loopback. The VNC fixture exercises raw RFB frames
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

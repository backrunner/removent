# CloudKit connection sync validation — 2026-09-30

This is the historical implementation check. For the subsequently provisioned
Developer ID helper, Production schema deployment and real CloudKit round trip,
see [Production validation — 2026-10-09](cloudkit-production-2026-10-09.md).
The limitations below describe the 2026-09-30 environment.

The local implementation is built and tested. **No real CloudKit exchange has
been verified.** This machine's device build stopped at Xcode's `No Accounts`
error, and the available wildcard iOS profile lacks iCloud and Push Notifications.
No cloud schema, production records or signing-account configuration was changed.

The current apps target macOS 26 and iOS/iPadOS 26. Older-system adapters, UI
fallbacks and the old-file reconciliation path have been removed.

## Completed checks

| Check | Observed result |
| --- | --- |
| Rust client/mobile/core library tests | 141 passed, 3 ignored. Includes account isolation, sign-out/offline edits, conflicts, late acknowledgements, deletions and redo-journal recovery. |
| Desktop GPUI sync settings test | Passed: enable/disable persists and a development executable without the packaged helper reports configuration status. |
| Shared Swift package | 5 passed: encrypted payload/system-field round trip, incompatible schema rejection, wrong record identity rejection, stale account callbacks and sent base-revision preservation. |
| iPhone simulator | 8 controller tests and 1 CloudKit settings UI test passed. |
| iPad simulator | The same 8 controller tests and 1 UI test passed against the final mobile sources. |
| Simulator visual inspection | Inspected the iPhone light and iPad dark settings captures. Standard Form rows, status text, controls and the native sheet toolbar were legible and aligned. These captures do not establish every accessibility size or conflict layout. |
| macOS development package | Built `dist/Removent.app` and the versioned ZIP with the embedded sync helper. `codesign --verify --deep --strict` passed for the ad-hoc package. The app, tray and sync helper Info.plist and executable deployment targets are verified as macOS 26. This is not Developer ID signing or notarization. |
| Real helper lifecycle, isolated data | Unprovisioned helper reports configuration, a second owner exits, disabling sync exits the helper, and a restarted helper exits when its parent closes. No CloudKit access is needed for this test. |
| Existing live tray/daemon lifecycle | All 5 checks passed: singleton ownership, recovery, explicit stop persistence, login toggles and relocated embedded tray behavior. |
| Rust Clippy | Passed for client, mobile, CLI and app, including all targets, with `-D warnings`. Cargo separately reports existing future-incompatibility notices in `block`/`proc-macro-error2`. |
| Scripts | 30 Python tests passed, 1 skipped; shell syntax, Python compilation and Git whitespace checks passed. Provisioning tests cover team, App ID, container, expiry, environment and distribution/development separation. |

After the macOS 26/iOS 26 cleanup, the shared Rust/Swift suites and iPhone/iPad tests
were rerun. The daemon IPC/readiness suites passed 9 tests; the tray integration
test passed with the required current status fields. The website type check,
strict bilingual documentation check, static build and all 18 Chromium/WebKit
browser tests passed. Website content was updated locally; it was not deployed.

## Reproduce local checks

```sh
cargo test -p removent-client -p removent-mobile -p removent-core --lib
cargo test -p removent-app cloud_sync_switch_persists_without_requiring_cloud_access
cargo clippy -p removent-client -p removent-mobile -p removent-cli -p removent-app --all-targets -- -D warnings
xcrun swift test --package-path packages/apple-cloud-sync
python3 -m unittest discover -s scripts -p 'test_*.py'
bash scripts/build/package.sh
python3 scripts/qa/test_cloud_sync_helper.py
codesign --verify --deep --strict dist/Removent.app
```

The helper lifecycle test requires an ad-hoc package without an embedded profile.
The two mobile test selections are `RemoventMobileTests` and
`RemoventMobileUITests/ConnectionTests/testCloudSyncConfigurationAndOptOut` in the
`RemoventMobile` scheme. Use an installed ARM64 iOS simulator. The UI test deliberately
reports unavailable CloudKit configuration, keeping it independent of cloud accounts.

## Still required before claiming cross-device sync

Follow [Apple setup](../architecture/cloudkit-sync-design.md#apple-setup-and-release), then verify:

1. Authorized Development profiles for both callers and the shared container;
   run create/rename/endpoint-change/delete from Mac to iPhone and in reverse.
2. Offline concurrent edits, conflicts, interrupted uploads, sign-out/sign-in and
   a different-account switch with actual CloudKit callbacks.
3. Push delivery and network/quota recovery on supported devices. Current local checks cover compilation and storage policy, not these
   server/runtime behaviors.
4. Deploy the encrypted `SavedConnection.payload` schema to Production; provision
   the Developer ID helper and mobile distribution target, then verify an actual
   Production Mac/TestFlight exchange.

Passwords and pairing are intentionally local in this release. Neither shared
connection metadata nor a passing simulation proves that a remote host is reachable
or that a receiving device has the credentials needed to connect.

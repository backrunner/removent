# Removent iOS submission materials

Public templates and evidence contain no private review contact. Build the private local package with:

```sh
rtk proxy python3 scripts/release/ios_store_materials.py --check
rtk proxy python3 scripts/release/ios_store_materials.py --package --private-config userdata/release/ios-review.private.json
```

The output lives in ignored dist/app-store-ios/. Screenshots and video are generated from the native client; source and validation limits are recorded in their manifests. A materials check is not signing, upload, App Review, or legal clearance. Current source version is 0.1.5 (6).


Included: `metadata.json`, bilingual descriptions and TestFlight text, English reviewer notes, Chinese review guide, data-flow/privacy/export/age-rating workpapers, dependency inventory and bundled acknowledgements. The ignored private ZIP adds the reviewer contact, evidence manifest, 20 screenshots and native session video. It does not include the website product credential.

Capture evidence uses iOS 27 simulators (iPhone 17 Pro and 13-inch iPad Pro) and the actual SwiftUI client. The isolated host fixture encodes a rendered documentation page and performs real RVP trust, PIN pairing, video decoding and keyboard transport. It does not capture a physical Mac desktop or inject input into macOS. The video is an App Review attachment, not an App Preview. Screenshot provenance, source hashes and lossless landscape orientation normalization are recorded in `dist/app-store-ios/screenshot-manifest.json`.

Run native captures with the `StoreCaptureTests` test class while the opt-in loopback `mobile_fixture` example is running. Disable Xcode diagnostic collection (`-collect-test-diagnostics never`) for repeatable result finalization. Export native screenshots with `xcrun xcresulttool export attachments`, preserving the original files as evidence; normalize PNG orientation metadata for landscape delivery without resizing or adding overlays. The release validator rejects wrong sizes, duplicates, unsafe paths, changed source/video hashes and private contact text in public files.

Current remaining conditions: real device/full desktop-host acceptance, production CloudKit acceptance, legal seller/territories, final privacy/export/age-rating answers, complete license clearance, distribution signing/Connect validation, and support ticket reply-delivery verification. OnFire intake is deployed and a Chinese production ticket has been verified; this does not prove email delivery. No build was uploaded or submitted for review.

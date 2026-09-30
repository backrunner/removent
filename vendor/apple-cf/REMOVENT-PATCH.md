Source: apple-cf 0.9.3, as published on crates.io. Original Apache-2.0 / MIT
licenses and source are retained. This directory is a Cargo patch, not app code.

Local changes:
- Select the Swift bridge target and SDK from Cargo's target (macOS, iOS device,
  or iOS simulator), instead of always building macOS objects.
- Remove SDKROOT from the SwiftPM subprocess environment so its manifest builds
  for the host; pass the target SDK explicitly for the library build.
- Declare iOS 17 in Package.swift and exclude three macOS-only XML/distributed
  notification entry points from the iOS bridge. Removent does not use them.

Remove this patch when an upstream release supports these targets. Validate both
the desktop codec tests and device/simulator links whenever updating it.

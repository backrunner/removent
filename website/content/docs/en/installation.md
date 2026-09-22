---
title: "Installation"
description: "Get Removent onto your Mac and prepare for your first session."
order: 1
---

## Requirements

- macOS 13 Ventura or later.
- An Apple silicon Mac (arm64).
- For native sessions, another Mac running Removent on a reachable LAN or a configured private relay.
- For VNC or RDP, an existing reachable server with supported authentication.

## Install from a release

Open [GitHub Releases](https://github.com/backrunner/removent/releases) and choose the latest published macOS arm64 DMG. Release availability and exact filenames are listed there.

Open the DMG and double-click **Removent.app → Install and Open**, or drag the app into **Applications**. Official distribution builds use Developer ID signing and notarization. Locally packaged ad-hoc builds are intended for development.

For a manual replacement, quit the main app and menu bar app and stop the background service first. The installer refuses to replace running code. Prefer the in-app updater for an existing installation.

## First launch

Open Removent from Applications. To share this Mac, use the menu bar's **Set Up Screen Sharing Permissions…** action. Grant Screen Recording and Accessibility, then restart the background service.

[Continue with permissions](/docs/permissions), then [connect your Macs](/docs/connecting).

## Updates

Removent checks for updates after launch and periodically afterwards. You can disable automatic checks in Settings. Available updates are announced; installation only begins when you ask.

The updater verifies the manifest's SHA-256 and Ed25519 signature before replacing the app, and retains the previous version for rollback. The update feed uses GitHub's latest stable release; it does not offer prereleases.

## Build it yourself

If no release is available for your needs, follow [Build from source](/docs/building). Source builds require Xcode and a Rust toolchain.

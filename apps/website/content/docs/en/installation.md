---
title: "Installation"
description: "Get Removent onto your Mac and prepare for your first session."
order: 1
---

## Requirements

- macOS 26 Tahoe or later.
- An Apple silicon Mac (arm64).
- For native sessions, another Mac running Removent on a reachable LAN or a configured private relay.
- For VNC or RDP, an existing reachable server with supported authentication.

## Install from a release

Open [GitHub Releases](https://github.com/backrunner/removent/releases) and choose the latest published macOS arm64 DMG. Release availability and exact filenames are listed there.

Open the DMG and double-click **Removent.app → Install and Open**, or drag the app into **Applications**. Official distribution builds use Developer ID signing and notarization. Locally packaged ad-hoc builds are intended for development.

For a manual replacement, quit the main app and menu bar app and stop the background service first. The installer refuses to replace running code. Prefer the in-app updater for an existing installation.

## Install from the terminal or SSH

The [installer script](https://raw.githubusercontent.com/backrunner/removent/main/scripts/install.sh) is publicly accessible from GitHub's `main` branch. Pushing script changes to `main` updates this URL; no repository clone is needed.

In a terminal on the target Mac, install the latest stable release:

```bash
curl -fsSL https://raw.githubusercontent.com/backrunner/removent/main/scripts/install.sh | bash
```

From another computer, install into the Mac user's Applications directory and enable background service startup at login (replace `user@mac`):

```bash
ssh user@mac 'curl -fsSL https://raw.githubusercontent.com/backrunner/removent/main/scripts/install.sh | bash -s -- --app-dir "$HOME/Applications" --login'
```

`--login` starts the daemon and enables startup at future user logins. Omit it to install only; `--start` starts without changing login preferences. Use `--version v0.1.4` to pin a published release or `--verify-only` to download and verify without installing. Run as the target Mac user, without sudo; `/Applications` may require administrator authorization, while `--app-dir "$HOME/Applications"` uses a user-owned directory.

The installer verifies SHA-256, component signatures and Gatekeeper acceptance before replacement, and retains the previous app for rollback. Quit the main app and menu bar app and finish remote sessions before upgrading. A previously running daemon is refreshed; an explicitly stopped daemon stays stopped unless you request startup. Data, sharing settings and login preferences are preserved.

Starting or refreshing the daemon requires the SSH user to have a logged-in graphical session. First-time Screen Recording and Accessibility approval still requires the Mac's System Settings. Installing the app does not grant these permissions or make a logged-out desktop available.

## First launch

Open Removent from Applications. To share this Mac, use the menu bar's **Set Up Screen Sharing Permissions…** action. Grant Screen Recording and Accessibility, then restart the background service.

[Continue with permissions](/docs/permissions), then [connect your Macs](/docs/connecting).

## Updates

Removent checks for updates after launch and periodically afterwards. You can disable automatic checks in Settings. Available updates are announced; installation only begins when you ask.

The updater verifies the manifest's SHA-256 and Ed25519 signature before replacing the app, and retains the previous version for rollback. The update feed uses GitHub's latest stable release; it does not offer prereleases.

## Build it yourself

If no release is available for your needs, follow [Build from source](/docs/building). Source builds require Xcode and a Rust toolchain.

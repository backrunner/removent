---
title: "Background hosting"
description: "Keep the host running independently of the desktop and menu bar apps."
order: 6
---

## Three independent processes

**Removent.app** is the controller and desktop interface. **removentd** owns hosting, capture, device identity, and input. **RemoventTray** provides menu bar controls, pairing PINs, and admission prompts.

Quitting the main window or menu bar app leaves the packaged background server running. Disabling sharing is a separate, persistent action.

## Prepare unattended access

1. Complete [daemon permission setup](/docs/permissions).
2. Pair each authorized controller while someone is present at the host.
3. Enable **Start Server at Login**.
4. Optionally enable **Show Menu Bar App at Login**. This is a separate setting.
5. Adjust system sleep settings for the intended use.
6. Close the desktop and menu bar apps, then test reconnection and a logout/login cycle on the actual host.

Trusted devices reconnect within approved grants. Unknown devices and new capabilities still need authorization.

## Manage from the CLI

```sh
REMOVENT_CLI=/Applications/Removent.app/Contents/MacOS/removent-cli
"$REMOVENT_CLI" daemon start
"$REMOVENT_CLI" daemon enable
"$REMOVENT_CLI" daemon status
"$REMOVENT_CLI" daemon login-on
"$REMOVENT_CLI" daemon service-status
```

`daemon login-off` removes future login registration without stopping the current service. `daemon disable` persists disabled sharing while retaining management access. `daemon stop` stops the process now but leaves the login preference unchanged.

Restarting or stopping the daemon interrupts active sessions.

## Availability boundaries

The host runs as a per-user LaunchAgent in a logged-in graphical session. It does not provide access to FileVault preboot, the initial login window, a logged-out user, or a sleeping Mac. Locked-session and display-less behavior needs validation on the target Mac and macOS version.

A background login registration cannot grant privacy access, unlock the Mac, or wake it by itself.

## Data and removal

The packaged app stores data in `~/Library/Application Support/removent/userdata`, including a `logs/` directory. `REMOVENT_DATA_DIR` overrides the data location.

Before removing background operation, disable the tray login item, run `daemon login-off`, then `daemon stop`. Removing the app does not remove device identity or settings.

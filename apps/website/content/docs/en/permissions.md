---
title: "macOS permissions"
description: "Give the background host the access it needs to share your screen."
order: 3
---

## Permissions for hosting

| Permission | Why it is needed |
| --- | --- |
| Screen Recording | Capture the host's display for a remote session |
| Accessibility | Deliver remote keyboard and mouse input |
| Local Network, when requested by macOS | Discover and connect to devices on your LAN |

Open **System Settings → Privacy & Security** to review access. A controller connecting to another machine does not need screen capture permission merely to view it.

## Set up the background process

Use **Set Up Screen Sharing Permissions…** in the Removent menu bar app. This requests access from the daemon that actually captures the screen and delivers input.

After granting access, choose **Restart Background Service**, then check the permission indicators again. Granting access to the main app alone does not prove that the background daemon has access.

## Check from the terminal

The installed app includes the CLI:

```sh
REMOVENT_CLI=/Applications/Removent.app/Contents/MacOS/removent-cli
"$REMOVENT_CLI" daemon permissions
"$REMOVENT_CLI" daemon status
```

Use the corresponding path under `~/Applications` if you installed there.

## If the screen is blank or input does not work

Check Screen Recording for blank or unavailable capture, and Accessibility when you can see the screen but cannot control it. Restart the background service after changing permissions.

Automatic background launches do not display consent dialogs. Complete setup interactively before depending on [unattended hosting](/docs/hosting).

## Login and sleep

Privacy grants do not unlock FileVault, sign into a logged-out account, or wake a sleeping Mac. The current native host requires a logged-in graphical user session.

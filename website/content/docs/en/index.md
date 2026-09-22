---
title: "Get started"
description: "Connect your Macs with a native remote desktop, on your LAN or through your own relay."
order: 0
---

Removent is a remote desktop for Apple silicon Macs running macOS 13 or later. It combines native Removent connections, a VNC viewer, and an RDP client in one app. The project is in early development.

## Your first connection

1. [Install Removent](/docs/installation) on both Macs.
2. On the Mac you want to control, [grant hosting permissions](/docs/permissions) and enable the host service in the menu bar.
3. Open Removent on your other Mac. Select the host from the device list, or add it by address.
4. Enter the PIN shown by the host and approve the requested access.

Once paired, a trusted device can reconnect within its approved capabilities. [Walk through the connection flow](/docs/connecting).

## Choose a protocol

| Connection | Use it for | What you need |
| --- | --- | --- |
| Removent (RVP) | Native Mac-to-Mac sessions | Removent on both Macs, pairing, and a reachable host |
| VNC / Apple Remote Desktop | Existing VNC or macOS Screen Sharing hosts | A supported server and its credentials |
| RDP | Windows remote desktops | A reachable RDP server and an authorized account |

Removent pairing, native audio, clipboard, and adaptive media belong to RVP. Compatibility connections have their own limits: see [VNC](/docs/vnc) and [RDP](/docs/rdp).

## Connect across networks

Native discovery works on your LAN. To reach a Mac on another network, configure a [private relay](/docs/relay). You run the relay on a VPS or Cloudflare Containers; no Removent hosted account is required.

## Prepare an unattended Mac

Complete pairing and privacy setup while you are at the host. Then configure the [background service](/docs/hosting). Hosting requires a logged-in graphical session; it does not provide FileVault preboot access.

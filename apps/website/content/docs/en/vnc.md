---
title: "VNC & Screen Sharing"
description: "Connect to VNC servers and macOS Screen Sharing from Removent."
order: 4
---

## Connect as a viewer

Choose **Add connection → VNC / Apple Remote Desktop**. Enter a hostname or IP address, the port (default `5900`), and the server's credentials. IPv4, IPv6, and custom ports are supported.

Standard VNC servers usually require a password. For Apple Remote Desktop or macOS Screen Sharing, use a macOS username and password.

## Supported authentication

Removent negotiates RFB 3.3, 3.7, and 3.8. It supports:

- None (type 1).
- VNC password (type 2).
- Apple ARD Diffie-Hellman/AES (type 30).

Apple type 35, VeNCrypt/TLS, RealVNC proprietary authentication, and UltraVNC MS-Logon are not implemented. Configure the server to offer a supported method.

## Display and input

The viewer supports Raw, CopyRect, Hextile, and desktop resizing. Keyboard, click, and scroll input stays ordered; consecutive pointer moves can be coalesced to the latest position.

Use **Control + Command + I** to inspect receive rate, pixel processing, and input queue information. Local input send time does not include the remote machine's response.

## Share Removent through VNC

The Removent host can optionally expose a VNC listener. It is disabled by default. Enable **Apple Remote Desktop / VNC** in Settings, set a password, then restart the host service.

An empty password selects unauthenticated RFB and should only be used on an isolated, trusted LAN. VNC does not inherit Removent's native mutual TLS or pairing; use a trusted network or a separately secured tunnel.

## Compatibility limits

The VNC path provides framebuffer display and keyboard/mouse input. Native Removent pairing, audio, clipboard, and adaptive media are available through [RVP](/docs/connecting), not this compatibility service.

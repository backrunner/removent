---
title: "Windows with RDP"
description: "Open a Windows remote desktop using the built-in RDP client."
order: 5
---

## Prepare Windows

Enable Remote Desktop on the Windows host and allow the intended account to log in. Your Windows edition and network policy must support incoming RDP. Confirm that the server is reachable from your Mac.

## Add the connection

1. Choose **Add connection → RDP**.
2. Enter the hostname or IP address. The default port is `3389`.
3. Enter your username and password, and a domain if your environment requires one.
4. Start the connection. A pending attempt can be cancelled.

Removent uses IronRDP for an in-app session with TLS, NLA/CredSSP, graphics, keyboard, and mouse input. The password entered in the connection dialog is not written to settings.

## Server certificates

Server certificates are verified by default. For a self-signed server, establish system trust or explicitly allow an untrusted certificate for that connection after verifying the host.

## Current scope

RDP is client-only. Removent does not act as an RDP server. RDP audio, clipboard, file redirection, and automatic reconnection are not currently implemented.

If the session fails, check the host address, Windows Remote Desktop settings, account permissions, certificate trust, and any firewall or VPN between the machines.

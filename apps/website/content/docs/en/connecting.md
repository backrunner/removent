---
title: "Connect your Macs"
description: "Discover a nearby Mac, pair securely, and open a native session."
order: 2
---

## Prepare the host

On the Mac you want to control, install Removent, [grant permissions](/docs/permissions), and enable sharing from the menu bar. Keep both Macs on the same reachable network for your first pairing.

## Discover or add a host

Removent uses mDNS to discover nearby hosts. Open the app on your controller and select the Mac in the device list.

Starting with v0.1.3-beta.1, Settings provides independent Removent, VNC, and RDP discovery controls. Removent defaults to on; VNC and RDP default to off. Compatibility discovery combines service announcements with standard-port probes on directly attached IPv4 subnets (VNC 5900, RDP 3389), including services that do not advertise. IPv6 and custom ports rely on service announcements or manual entry.

Scans exclude this machine, point-to-point VPNs, and networks that are not directly attached. They confirm protocol handshakes without logging in. Disabling a protocol stops its discovery and removes its list entries. Selecting a VNC/RDP device prefills its address; you still enter credentials, and discovery does not establish trust.

If discovery does not find it, choose **Add connection → Removent** and enter its address. Discovery may be blocked between guest networks, VLANs, or networks with client isolation even when a manually entered address is reachable.

The protocol is selected explicitly. Choosing an unusual port does not change a Removent connection into VNC or RDP.

## Pair and approve

Enter the PIN displayed by the host. The host must approve the requested access. Native RVP uses SPAKE2 pairing and pinned device certificates with mutual TLS.

After pairing, trusted devices can reconnect within their granted capabilities. Additional capabilities still require approval, and a host configured to deny incoming connections continues to reject them.

## During a session

Move the pointer to the top edge to reveal the session toolbar. The remote image remains the focus of the window.

Performance information is hidden by default. Use the toolbar's gauge icon or **Control + Command + I** to show it. **Control + Command + Escape** is the local escape shortcut.

[Picture quality](/docs/quality) explains how native sessions adapt to changing network conditions.

## When the Macs are on different networks

Configure [a private relay](/docs/relay), then select **Connect through relay** in the Removent connection form. A relay provides network reachability; device pairing and host authorization still apply.

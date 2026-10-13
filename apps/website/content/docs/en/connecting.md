---
title: "Connect your Macs"
description: "Discover a nearby Mac, pair securely, and open a native session."
order: 2
---

## Prepare the host

On the Mac you want to control, install Removent, [grant permissions](/docs/permissions), and enable sharing from the menu bar. Keep both Macs on the same reachable network for your first pairing.

## Discover or add a host

Removent uses mDNS to discover nearby hosts. Open the app on your controller and select the Mac in the device list.

Settings provides independent Removent, VNC, and RDP discovery controls. Removent defaults to on; VNC and RDP default to off. Starting with v0.1.4, desktop and mobile use Bonjour/mDNS service announcements only. Add unadvertised servers manually, including custom ports.

Disabling a protocol stops its discovery and removes its list entries. Selecting a VNC/RDP device prefills its address; you still enter credentials, and discovery does not establish trust.

Starting with v0.1.5, nearby lists exclude this computer for all three protocols using loopback and assigned interface addresses. Local addresses are rechecked when the network changes. A remote computer with the same display name remains visible.

If discovery does not find it, choose **Add connection → Removent** and enter its address. Discovery may be blocked between guest networks, VLANs, or networks with client isolation even when a manually entered address is reachable.

The protocol is selected explicitly. Choosing an unusual port does not change a Removent connection into VNC or RDP.

## Pair and approve

Enter the PIN displayed by the host. The host must approve the requested access. Native RVP uses SPAKE2 pairing and pinned device certificates with mutual TLS.

After pairing, trusted devices can reconnect within their granted capabilities. Additional capabilities still require approval, and a host configured to deny incoming connections continues to reject them.

## Choose host authentication

The controlled Mac selects the policy in **Settings → Security**. Both desktop and the iPhone/iPad controller follow the host's policy automatically:

- **Pairing code**: enter the host's six-digit PIN, or a host-generated one-time 12-digit connection code. The host chooses whether to remember a device or require pairing on every connection.
- **Access password**: enter the password configured on the host. Previously paired devices also need it.
- **Authenticator OTP**: enter the current six-digit code from the configured authenticator. Codes expire and cannot be reused.
- **No authentication**: no credential prompt. RVP still uses encrypted transport and host certificate verification.

Password and OTP use SPAKE2 proofs and signed mutual confirmation; they are not sent as plaintext credentials. These policies work with direct connections and private relays. A relay's access password and device authorization are checked separately from the host's credentials. Disabling incoming connections still rejects every mode. Changing the host policy invalidates quick resume under the old policy; reconnect and enter the new credential.

## During a session

Move the pointer to the top edge to reveal the session toolbar. The remote image remains the focus of the window.

Performance information is hidden by default. Use the toolbar's gauge icon or **Control + Command + I** to show it. **Control + Command + Escape** is the local escape shortcut.

[Picture quality](/docs/quality) explains how native sessions adapt to changing network conditions.

## When the Macs are on different networks

Configure [a private relay](/docs/relay), then select **Connect through relay** in the Removent connection form. A relay provides network reachability; device pairing and host authorization still apply.

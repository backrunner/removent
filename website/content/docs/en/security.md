---
title: "Device trust"
description: "Understand pairing, permission grants, certificates, and relay access."
order: 9
---

## Native device identity

Native Removent sessions run RVP over QUIC with mutual TLS and pinned device certificates. Initial PIN pairing uses SPAKE2. The host retains the device identity and approved capability grants.

Access is controlled per connection. Trusting a device does not automatically approve every future capability it may request.

## A relay is a transport

Relay access and remote desktop access are separate checks. The relay uses independent host and controller credentials and device-key proofs. The native session inside the tunnel still performs RVP pairing and authorization.

VPS / QUIC requires a pinned relay certificate fingerprint. Cloudflare / HTTPS uses certificate authority and hostname verification. Share fingerprints through a trusted channel.

The relay can observe IP addresses, rooms, packet timing, and sizes. It cannot read the end-to-end encrypted native screen, keyboard input, or clipboard contents.

## Keep identity and credentials private

Desktop relay credentials are stored in macOS Keychain, rather than bookmark JSON. Device identity and settings live in the app's local data directory. Do not publish or commit that directory.

Generated relay configuration includes credential files. Back it up securely, preserve the relay identity across updates, and use separate host, controller, and admin credentials where applicable.

## Compatibility protocols

VNC uses the server's selected RFB authentication and does not inherit native Removent encryption or pairing. An empty VNC password allows unauthenticated access.

RDP verifies server certificates by default and supports NLA/CredSSP. Only allow an untrusted certificate for a specific connection after verifying the server.

Read the [VNC](/docs/vnc) and [RDP](/docs/rdp) guides before connecting to those services.

## When retiring a Mac

Disable sharing and background login items before retiring a host. Removing the app does not remove its identity or settings. Treat local identity data as sensitive when transferring or erasing a machine.

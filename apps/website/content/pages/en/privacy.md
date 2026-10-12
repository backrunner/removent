---
title: "Privacy policy"
description: "How Alkinum handles Removent connections, optional iCloud sync, and support requests."
---

Effective date: 12 October 2026. Removent is provided under the **Alkinum** brand. Contact us using our [support form](/support).

## Connections on iPhone, iPad, and Mac

Removent does not require a hosted Removent account. Connection names, addresses, device identities, trusted certificates, and preferences are stored on your device. Mobile passwords are stored in the iOS Keychain, rather than in connection files. Device pairing and credential storage remain local. Removing the app does not necessarily remove Keychain entries or cloud data.

A connection transmits the remote computer's screen and your input to the computer you choose. Native Removent connections can also transmit remote audio and text clipboard content when enabled. Session options apply to the next connection. The mobile app plays received audio; it does not request microphone recording access. Clipboard access is for transferring text during your chosen session. Do not connect to a computer without its owner's permission.

Local Network permission lets the app discover and reach computers on your network. It does not grant access to your photos, contacts, location, or microphone. You can change discovery preferences in Removent and Local Network permission in system Settings.

Native RVP content is encrypted end to end. A private relay routes native connection traffic and may observe addresses, connection times, and infrastructure logs. The relay operator's policies apply. VNC and RDP use their own security mechanisms; native RVP encryption guarantees do not apply to them. Removent displays certificate trust decisions, and verification exceptions reduce identity protection.

## Optional iCloud sync

Saved-connection sync is off unless you enable it. It uses your Apple account and the private CloudKit database for `iCloud.com.alkinum.removent`. Saved names, endpoints, routing settings, and verified host identity information can sync between your devices. Passwords, pairing secrets, Keychain references, and certificate-verification exceptions do not sync.

Turning sync off keeps local connections and does not erase existing cloud records. Deleting a saved connection while sync is enabled queues a deletion for your other devices; deletion metadata can remain for synchronization. Account and network availability can delay propagation. Apple provides iCloud services under its own policies. Use the system's iCloud storage controls to manage any Removent cloud data offered there.

## Retention and deletion

Local connection information remains until you delete it. Editing a connection preserves its saved password unless you replace it; deleting a connection removes its associated credential when no longer referenced. Turning discovery, clipboard, audio, or sync off withdraws permission for that feature, subject to the next-connection behavior described above. Disconnect to end the current session.

Desktop diagnostic logs are local and can include technical device or network information. Do not publish raw logs without reviewing them. Mobile connection errors stay in the app; there is no advertising or analytics SDK and no automatic upload of remote screens or credentials to Alkinum.

## Support requests

Submitting our support form sends your email, topic, subject, message, language, and optional app/device details to Alkinum's OnFire support system. These are used to investigate and reply to your request. Cloudflare Turnstile performs abuse checks; the website's infrastructure uses request information such as IP addresses for security and rate limiting. No attachments are requested. Do not submit passwords, connection or pairing codes, private screen contents, or Apple ID credentials.

Support records are kept while needed to handle the request and maintain its support history. You can contact us to request access, correction, or deletion; any required retention will be explained when applicable. Do not send sensitive information in a deletion request.

## This website and external services

Pages, fonts, icons, and local search are served with the site. Your theme choice is stored in browser local storage. The support page loads Cloudflare Turnstile; other pages do not load an advertising or analytics SDK. Hosting infrastructure may process normal request information to serve and protect the site.

Desktop downloads and update checks use GitHub. iOS updates are delivered by Apple. Following external links is subject to the destination's policies. Refer to [device trust](/docs/security) and [support](/support) for security guidance.

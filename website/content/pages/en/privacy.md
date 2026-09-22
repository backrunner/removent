---
title: "Privacy, in plain terms."
description: "What this website loads, and where Removent connections and credentials live."
---

## This website

This documentation site is statically generated. It does not include an analytics SDK, an advertising SDK, a support form, or a hosted AI service. Fonts, icons, screenshots, and search data are served with the site.

Search runs locally in your browser. The appearance toggle stores your theme preference in browser local storage. The selected language is reflected in the page URL.

The hosting provider may process ordinary request information such as IP addresses and user agents to serve and protect the site.

## Downloads and external links

Downloads, source code, and release notes link to GitHub. When you follow an external link, the destination provider's policies apply. This site does not ask for a Removent account.

## The app and your connections

Native LAN connections do not require a hosted Removent account. Device identities and settings are stored locally. Desktop relay credentials are stored in macOS Keychain.

If you configure a private relay, your provider can observe infrastructure-level connection information. Native RVP content remains encrypted end to end; relay access does not grant desktop access. VNC and RDP have different security properties.

Automatic update checks contact GitHub and can be disabled in Settings. Read [Device trust](/docs/security) for pairing, certificate verification, and compatibility protocol details.

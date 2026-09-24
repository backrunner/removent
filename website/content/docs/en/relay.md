---
title: "Private relay"
description: "Deploy a private relay on Linux, macOS, or Cloudflare, with updates and local logs."
order: 7
---

The relay carries encrypted native RVP traffic between networks. Device pairing, capability grants, and mutual TLS remain end to end. Your relay can observe endpoint IPs, room names, traffic timing, and sizes, but does not decode your desktop.

## Install on a VPS

Use Linux x86_64 or ARM64 with systemd 247 or newer, such as Ubuntu 22.04+ or Debian 12+. The server needs curl and tar; it does not need a Rust toolchain or Docker.

```sh
curl -fsSL https://raw.githubusercontent.com/backrunner/removent/main/scripts/install_relay.sh | sh
```

The installer downloads a prebuilt binary, verifies SHA-256 and its version, then opens the native setup wizard. Enter your public address, such as `removent://relay.example.com:48700`, room, and optional source networks. Setup generates separate host and controller credentials and starts an unprivileged systemd service. Allow the selected UDP port in your VPS firewall.

The selected stable GitHub release must include relay binaries. Releases published before these assets are introduced cannot be installed with this command.

To review the script or install a specific version first:

```sh
curl -fsSL https://raw.githubusercontent.com/backrunner/removent/main/scripts/install_relay.sh -o install_relay.sh
sh install_relay.sh --version v0.1.2 --no-setup
sudo removent-relay setup --address removent://relay.example.com:48700
```

## Try the beta

Automatic updates, `check-update`, and rotating local relay logs are available starting with **v0.1.3-beta.1**. The installer defaults to stable releases. To try these features, select the beta explicitly:

```sh
curl -fsSL https://raw.githubusercontent.com/backrunner/removent/v0.1.3-beta.1/scripts/install_relay.sh | sh -s -- --version v0.1.3-beta.1
```

[Read the beta release notes and download](https://github.com/backrunner/removent/releases/tag/v0.1.3-beta.1). Restart an existing service after installation. Later betas also require manual installation. Automatic updates follow stable releases, including upgrading a beta to a newer stable version.

## Host a relay on a Mac

The same curl command installs a complete relay on **macOS 13+**, with one Universal binary for **Apple Silicon and Intel**. Run the installer as your normal login user. It requests sudo only when writing the binary to `/usr/local/bin`; setup and service management run without sudo.

```sh
removent-relay status
removent-relay start
removent-relay stop
removent-relay restart
removent-relay logs --follow
removent-relay enable
removent-relay disable
```

The user LaunchAgent starts at login and recovers after a crash. `enable` and `disable` affect future logins without interrupting an active relay. `stop` unloads the running job so launchd will not immediately restart it. Logout stops the relay; a sleeping Mac cannot forward traffic. Keep the Mac logged in and awake, and configure firewall access and router UDP forwarding when needed.

Private configuration, connection profiles, identity, and logs live in `~/Library/Application Support/removent-relay`. This service is separate from the desktop app's background daemon and needs no screen-recording or accessibility permission. macOS profile export uses your own directory:

```sh
removent-relay check
removent-relay fingerprint
removent-relay export host --output "$HOME/relay-host.toml"
removent-relay export client --output "$HOME/relay-client.toml" --host-fingerprint VERIFIED_TARGET_MAC_FINGERPRINT
```

Repeat the installer to upgrade, then run `removent-relay restart`. Installation preserves credentials, identity, login startup preferences, and whether the service is stopped. `uninstall` removes the LaunchAgent while retaining data and the binary. Intel support here applies to the standalone relay; the desktop app has its own system requirements.

## Manage a Linux service

```sh
sudo removent-relay start
removent-relay status
sudo removent-relay logs --follow
sudo removent-relay stop
sudo removent-relay restart
```

Use `enable` and `disable` to control boot startup separately. Stopping a service does not disable its next boot startup. `check` validates the configuration; `fingerprint` prints the existing relay certificate fingerprint. Use sudo to read private configuration or make service changes.

Run the installer again to upgrade, then explicitly `restart` to run the new binary. Upgrades preserve configuration, credentials, and identity. Restarting disconnects active sessions. `uninstall` removes the service while retaining those files and the binary.

## Connect your Macs

Linux setup creates private profiles in `/etc/removent-relay`; macOS uses the directory above. Both already contain the relay certificate fingerprint. Export them through the CLI:

```sh
sudo removent-relay export host --output /root/relay-host.toml
sudo removent-relay export client --output /root/relay-client.toml --host-fingerprint VERIFIED_TARGET_MAC_FINGERPRINT
```

Replace `VERIFIED_TARGET_MAC_FINGERPRINT` with the full fingerprint from the target Mac's `removent-cli identity` output, verified through a trusted channel.

1. Securely transfer the host profile into the target Mac's daemon data directory, available from the tray's **Open Data Directory** action. Set its owner to the Mac user and permissions to 0600, then restart the background service.
2. On the controller, choose **Add connection → Removent → Connect through relay → VPS / QUIC**.
3. Enter the address, room, client credential, relay fingerprint, and target Mac fingerprint from your verified client profile.
4. Complete device pairing and approve access before leaving the host unattended.

Host and controller credentials are separate; distribute only the appropriate profile. The desktop stores relay credentials in macOS Keychain. Back up `/etc/removent-relay` and the contents of `/var/lib/removent-relay` on Linux, or the complete macOS relay directory, privately. Never regenerate the relay identity while continuing to use an old pin.

## Updates and local logs

### Enable or disable automatic updates

The relay can install signed stable releases automatically. This defaults to off. Edit `/etc/removent-relay/server.toml` on Linux or `~/Library/Application Support/removent-relay/server.toml` on macOS:

```toml
[updates]
enabled = true
check_interval_secs = 86400
```

Apply the change with `sudo removent-relay restart` on Linux or `removent-relay restart` on macOS. Enabled services check 30 seconds after starting, then at the configured interval, which must be 3600–604800 seconds. To disable automatic updates, set `enabled = false` and restart.

Downloads are checked against an Ed25519 release signature and SHA-256 hashes of both archive and binary. macOS also verifies Developer ID. The verified binary checks the existing configuration before restarting the relay, briefly interrupting connections. A failed check, download, or verification leaves the running version in place.

### Manual checks and version selection

Manual checks work independently of the automatic-update setting. They do not download or install a binary:

```sh
# Default macOS service
removent-relay check-update
# Linux: read private configuration and the installed service version
sudo removent-relay check-update --config /etc/removent-relay/server.toml
# Another instance
removent-relay check-update --config /path/to/server.toml
```

The command reads accessible local service configuration by default; without it, the command compares the CLI's own version. The stable feed must provide signed `relay-latest-<platform>.json` manifests. v0.1.2 has no such manifests. Until the feed includes them, a check reports a fetch error without affecting forwarding.

Updates live under `identity_dir/updates`; the system CLI remains the launcher, so the background process needs no system-directory write permission. Disabling updates preserves the installed version, configuration, credentials, and identity. To downgrade, stop the service, disable automatic updates, back up and move aside `identity_dir/updates`, install the selected version, then start the service.

### Log locations and retention

The relay's main log is `identity_dir/logs/removent-relay.log`. Default locations:

| Component | Log file |
| --- | --- |
| Linux relay | `/var/lib/removent-relay/logs/removent-relay.log` |
| macOS relay | `~/Library/Application Support/removent-relay/data/logs/removent-relay.log` |
| Desktop client, daemon, client CLI | `logs/removent.log`, `logs/removentd.log`, and `logs/removent-cli.log` in the data directory |

Each file is limited to 8 MiB and retains three backups, `.log.1`–`.log.3`, for approximately 32 MiB per component. Log files use mode 0600. The ten most recent crash reports are retained separately. Find the desktop data directory using the tray's **Open Data Directory**, or override it with `REMOVENT_DATA_DIR`.

On macOS, `removent-relay logs --follow` follows the local relay log. On Linux, that command reads the systemd journal; rotating local files are also retained. Use `sudo tail -F /var/lib/removent-relay/logs/removent-relay.log` to follow the Linux file.

## Cloudflare Containers

Cloudflare uses HTTPS / WebSocket on port 443. Initial infrastructure deployment requires a Containers-enabled account, Node 24+, Python 3.11+, and Docker able to build Linux amd64. Follow the [Cloudflare deployment guide](https://github.com/backrunner/removent/blob/main/docs/cloudflare-relay.md) for the Worker and container setup.

Cloudflare deployment, lifecycle, and upgrades use the repository scripts and management API. Its dashboard provides infrastructure and logs; installing the native relay CLI is unnecessary. From the source checkout:

```sh
bash scripts/deploy_relay.sh start
bash scripts/deploy_relay.sh status
bash scripts/deploy_relay.sh stop
bash scripts/deploy_relay.sh deploy
```

`deploy` rebuilds and deploys the container, leaving it stopped until an explicit `start`. Containers do not run the relay updater. Their disks are ephemeral; Cloudflare manages log retention. A stop through the script or management API persists; host retries cannot restart a stopped container. An enabled container can sleep while idle and wake for a controller. View container logs in Cloudflare's dashboard. Infrastructure charges follow your hosting provider's terms.

## Configuration reference

The [installation and configuration guide](https://github.com/backrunner/removent/blob/main/docs/relay-quick-deploy.md) covers source allowlists, unattended installation, configuration files, upgrades, and standalone foreground operation. See the [VPS reference](https://github.com/backrunner/removent/blob/main/docs/private-relay.md) for protocol and trust details.

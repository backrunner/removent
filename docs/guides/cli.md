# CLI guide

[简体中文](cli.zh-CN.md) | [Back to Removent](../../README.md)

The bundled CLI manages this Mac's settings and background service. It uses the
same private data directory and settings lock as the desktop and daemon.

```sh
REMOVENT_CLI="/Applications/Removent.app/Contents/MacOS/removent-cli"
"$REMOVENT_CLI" --version
"$REMOVENT_CLI" --help
"$REMOVENT_CLI" settings --help
```

For a per-user installation, replace `/Applications` with `$HOME/Applications`.
Source builds use `target/debug/removent-cli`; their default data directory is
`userdata`, while `scripts/dev.sh` uses `userdata/dev`. Set `REMOVENT_DATA_DIR`
explicitly when working with a custom or development instance.


## Inline terminal interface

Run `removent-cli` without arguments in a terminal, or use `removent-cli interactive`,
to open the inline keyboard menu. It keeps shell scrollback and never enters a
full-screen alternate buffer. Use ↑/↓ to select, Enter to open, and Esc or `q` to
return. The menu covers service management, pairing, settings, identity and ping.
Settings offer boolean/enum choices and hidden, confirmed secret entry. Saving or
resetting settings requires confirmation; service operations that can interrupt
sessions explain the impact before proceeding. Direct commands remain noninteractive.

Service, pairing and identity commands display readable results in a terminal.
Their redirected output retains the existing JSON schema, including tray integration.
`--json` explicitly requests machine output; `--human` requests readable output even
in a pipe. These global flags can appear before or after the command. They are
mutually exclusive. Settings and ping preserve their existing text defaults in pipes;
use `--json` for JSON settings records or a single ping statistics object. Pairing
watch uses JSON Lines in JSON mode and readable lifecycle events otherwise.

Menus require stdin, stdout and stderr attached to a terminal; `TERM=dumb` disables
automatic menus. No-argument calls in scripts still show help. `NO_COLOR` disables
result styling, and redirected output contains no cursor controls. Narrow-terminal
results wrap without dropping values. UI language follows the saved language setting.

```sh
"$REMOVENT_CLI" interactive
"$REMOVENT_CLI" pairing show
"$REMOVENT_CLI" daemon status --json
"$REMOVENT_CLI" daemon status --human > status.txt
"$REMOVENT_CLI" ping 192.168.1.10:48688 --pin 123456 --json
```

The ping prompt hides the PIN and restores terminal input on cancellation. Scripts
must supply `--pin` for pairing-code hosts. Diagnostics remain in local rotating logs;
`--verbose` explicitly mirrors them to stderr.


The interface uses a shared set of terminal components: bordered result cards,
separate status badges, a searchable selector with current values and category
context, editable inputs, a two-choice confirmation, and a delayed progress
indicator. Type to filter actions or settings (including their CLI keys). Esc first
clears an active filter, then returns. Enum pickers start on the current choice.
Text inputs support arrow keys, Home/End, Backspace/Delete and Ctrl+U; bracketed
paste cannot submit the form. Invalid values keep the draft in place, and a review
card shows old and new values before saving. Secrets use a fixed hidden-input
indicator. Terminal colors follow the active palette; `NO_COLOR` retains layout
and keyboard focus without ANSI colors. A small window requests more room before
accepting an action it cannot show. Resizing preserves prior snapshots in scrollback.


## Read and change settings

```sh
"$REMOVENT_CLI" settings list                    # Keys, values, accepted values and effects
"$REMOVENT_CLI" settings get                     # Current values
"$REMOVENT_CLI" settings get host_port           # One value
"$REMOVENT_CLI" settings path                    # The settings.toml in use
"$REMOVENT_CLI" settings set device_name "Office Mac"
"$REMOVENT_CLI" settings set discovery.vnc true
"$REMOVENT_CLI" settings set theme dark
"$REMOVENT_CLI" settings set update_channel beta
"$REMOVENT_CLI" settings reset theme             # Reset one key only
```

Boolean values accept `true`/`false` and `on`/`off`. Enum values ignore case,
hyphens and underscores, so `high-quality` selects `HighQuality` and `zh-CN`
selects `ZhCn`. Empty strings are passed as `""`.

Each edit re-reads the latest settings while holding the shared file lock,
validates the result, then atomically replaces the file with permissions `0600`.
Invalid values leave the previous file intact. A malformed configuration is
reported without moving or resetting the file or echoing its contents.

## Available settings

| Key | Accepted values | Effect |
| --- | --- | --- |
| `device_name` | Non-empty text, up to 63 UTF-8 bytes | Host reload; reopen desktop to refresh its displayed name |
| `language` | `System`, `En`, `ZhCn` | Reopen desktop; the next CLI command uses the new language |
| `theme` | `System`, `Light`, `Dark` | Reopen desktop |
| `discovery.removent` | Boolean | Reopen desktop |
| `discovery.vnc` | Boolean | Reopen desktop |
| `discovery.rdp` | Boolean | Reopen desktop |
| `host_enabled` | Boolean | Host reload |
| `host_port` | `1`–`65535`, UDP | Host reload |
| `admission` | `AlwaysAsk`, `TrustedAuto`, `DenyAll` | Host reload |
| `paired_only` | Boolean | Host reload |
| `authentication.mode` | `system_account`, `pairing_code`, `password`, `otp`, `none` | Host reload |
| `authentication.pairing_policy` | `remember_device`, `every_connection` | Host reload |
| `authentication.password` | Text up to 1024 bytes, via `--stdin` | Host reload |
| `authentication.otp_secret` | Valid Base32 secret, via `--stdin` | Host reload |
| `window_server_capture` | Boolean, opt-in legacy capture | Host reload |
| `vnc_enabled` | Boolean | Host reload |
| `vnc_port` | `1`–`65535`, TCP | Host reload |
| `vnc_password` | Text up to 1024 bytes, via `--stdin` | Host reload |
| `audio_enabled_default` | Boolean | Reopen desktop |
| `clipboard_clear_after_secs` | `0`–`4294967295`; `0` disables clearing | Reopen desktop |
| `video_quality` | `Auto`, `Smooth`, `Balanced`, `HighQuality`, `Extreme` | Reopen desktop |
| `update_check_enabled` | Boolean | Reopen desktop |
| `update_channel` | `stable`, `beta` | Reopen desktop |
| `update_endpoint` | HTTPS URL without credentials or fragment; empty uses default | Reopen desktop |

`settings` edits local preferences. Login registration and permissions are
managed with `daemon` commands below. Relay routes and iCloud provisioning have
separate configuration; see the [relay guide](private-relay.md) and
[iCloud design](../architecture/cloudkit-sync-design.md).

## Passwords and OTP

Passwords and OTP secrets are never returned by `list`, `get`, `set`, or JSON
output. Those commands show only whether a value is configured. Sensitive values
must come through stdin, so they are not passed as process arguments:

```sh
"$REMOVENT_CLI" settings set authentication.password --stdin < /private/path/password.txt
"$REMOVENT_CLI" settings set authentication.mode password
```

The input must be UTF-8 text no larger than 4096 bytes. One trailing line ending
is removed; spaces are preserved. Each setting also enforces its own length
limit. `--stdin` expects a pipe or file, not an interactive terminal.

For OTP, provision an authenticator with the same Base32 secret, then set
`authentication.otp_secret --stdin` before switching `authentication.mode` to
`otp`. Desktop Security settings can generate the secret and provisioning QR code.

Switch to `pairing_code` or `system_account` before resetting a password or OTP secret required
by the active mode. The CLI rejects an incomplete authentication configuration.
`system_account` authenticates against the host Mac and does not store the system password in these settings.
`none` permits connections without credentials; use it only when that is the
access policy you intend. An empty VNC password selects unauthenticated VNC.

## When changes take effect

Host settings send `ReloadSettings` to an online daemon after saving. Changes to
listener ports, names, sharing or authentication restart the hosting runner as
needed and can interrupt active remote sessions. Authentication changes also
clear pairing invitations and session resume state. Desktop-only changes do not
restart the host.

When the daemon is offline, saving succeeds and the settings apply when it next
starts. This does not start the service or change an explicit service-stop intent.
When a daemon rejects or fails to acknowledge a reload, the CLI reports that the
file **was saved**, exits with code `1`, and suggests `daemon reload`. Do not assume
a reload acknowledgement proves that the listener is ready; check `daemon status`
for `host_ready` and `host_error`.

## JSON and exit codes

Add `--json` to any settings command. `list` and `get` without a key return an
array of setting records; `get <key>` returns one record. Each includes `key`,
`value`, `type`, `accepted`, `secret`, and `apply`.

A secret value looks like `{"redacted":true,"configured":true}`. A successful
edit returns `saved`, `setting`, and `application`. `application` is one of
`desktop_restart`, `daemon_offline`, `reload_requested`, or `reload_failed`.

```sh
"$REMOVENT_CLI" settings get host_port --json
"$REMOVENT_CLI" settings set host_port 48688 --json
```

| Exit code | Meaning |
| --- | --- |
| `0` | Command succeeded; offline settings edits can also succeed |
| `1` | Runtime failure; a settings reload failure may follow a successful save |
| `2` | Invalid command, setting, value or incomplete authentication configuration |

Service, pairing and identity commands retain their existing JSON outputs when
redirected or when `--json` is specified. Terminal output is readable by default.
Diagnostics normally go to rotating local log files, with errors on stderr.
Use `--verbose` to also show logs and source error causes in the terminal;
`RUST_LOG` controls the diagnostic level.

## Service and pairing

```sh
"$REMOVENT_CLI" daemon status          # Sharing, listener readiness, sessions and permissions
"$REMOVENT_CLI" daemon service-status  # Process reachability and launch-at-login state
"$REMOVENT_CLI" daemon start
"$REMOVENT_CLI" daemon stop
"$REMOVENT_CLI" daemon restart
"$REMOVENT_CLI" daemon enable          # Persist sharing enabled
"$REMOVENT_CLI" daemon disable         # Persist sharing disabled, keep IPC running
"$REMOVENT_CLI" daemon reload          # Retry loading settings already saved on disk
"$REMOVENT_CLI" daemon login-on
"$REMOVENT_CLI" daemon login-off
"$REMOVENT_CLI" daemon permissions     # Request system privacy grants interactively
"$REMOVENT_CLI" pairing generate  # Requires pairing-code authentication and sharing enabled
"$REMOVENT_CLI" pairing show
"$REMOVENT_CLI" pairing watch          # Live events; --json for JSON Lines; Ctrl+C stops watching
"$REMOVENT_CLI" pairing revoke
```

Service startup, shutdown and login registration use macOS launchd. See the
[background service guide](macos-background-service.md) for lifecycle and
permission details. Use `ping --help` for direct or private-relay connection
checks, and `identity` for the public device fingerprint and key. The current
`ping` session probe supports pairing-code and no-authentication hosts only; use
the desktop or mobile controller to verify connections to a host using the default
system-account mode, an access password or OTP. It does not bypass host authentication.

# Local diagnostics

Open **Settings → System → Local diagnostics → Open Logs Folder**. The app saves
connection stages, failures, session lifecycle events, and startup information
automatically. No upload service is involved.

Default paths:

- Installed builds: `~/Library/Application Support/removent/userdata/logs/`
- Source/debug builds: `<workspace>/userdata/logs/`
- With `REMOVENT_DATA_DIR`: `<REMOVENT_DATA_DIR>/logs/`

`removent.log` contains app/client events; `removentd.log` contains background
hosting events. Each has up to three older files (`.log.1` newest through
`.log.3` oldest). Rotation happens while running, before a write would exceed
8 MiB, so new logs use up to 32 MiB per component. Pre-existing larger files
age out through the same rotation. Records are capped at 64 KiB. Writes are
serialized across threads/processes, and reopen the current file after rotation.
Every completed event is written to the OS without waiting for application exit.
This is not a guarantee against disk failure or sudden power loss.

`panics/` retains the ten most recent crash reports, each capped at 64 KiB,
including version, process/thread, source location, and a stack trace. Panic
payloads are omitted because they can contain arbitrary user data. Reports are
best effort; process termination, native crashes, and power loss do not invoke
the Rust panic hook. Diagnostic directories use mode `0700` and files `0600`.
If the log directory is unavailable, diagnostics continue on stderr and report
the file-writing error there.

## Diagnosing a connection

Reproduce the issue, then inspect the end of `removent.log`. A connection has a
`client_connection` span with an `attempt` number, protocol, and destination.
Progress records add the stage and elapsed milliseconds. Success, failure,
remote closure, and local cancellation are recorded. The process startup record
includes a PID, making attempts distinguishable across launches.

For VNC/Apple Screen Sharing, the log also records the selected security method
and whether the server advertises Apple's protocol. A timeout identifies the
failed step, for example `server greeting` or `authentication challenge`. This
lets us distinguish TCP reachability from protocol or authentication problems.

For more detailed diagnostics, launch the executable with a scoped filter:

```sh
RUST_LOG=info,removent_client=debug,removent_net=debug /Applications/Removent.app/Contents/MacOS/removent
```

`RUST_LOG` applies to both console and file output. The default is
`info,quinn=warn,rustls=warn`. For discovery-only investigation, add
`mdns_sd=trace,removent_net::discovery=debug` temporarily, then restart normally
when finished.

## Privacy and implementation

Connection logging explicitly selects address, protocol, stage, codec, duration,
and error fields. It does not dump connection requests, account credentials,
settings, PINs, clipboard contents, or keyboard input. The field formatter also
redacts structured password, PIN, token, secret, authorization, cookie, clipboard,
and credentials fields in both events and spans. This is defense in depth,
not a general sanitizer for secrets interpolated into arbitrary messages; new
callsites must preserve the same rule, including when debug logging is enabled.

Logs may contain LAN addresses, device names, local paths, and remote error
text. To share diagnostics, select the relevant log/crash files instead of the
whole data directory, which also holds identity keys and settings.

Regression tests cover runtime rotation, bounded history, concurrent independent
writers through rotation, level filtering, sensitive event/span fields, oversized
UTF-8 events, file permissions, unavailable directories, and the real panic hook
in a separate process.

Validation on 2026-09-08: the isolated staged source passed 125 app, client, and
core tests (33 + 45 + 47). App and daemon builds passed. A local Apple-banner VNC
fixture selected security type 30, then disconnected before the DH challenge;
the on-disk log recorded progress and the EOF error without the supplied account
or password.

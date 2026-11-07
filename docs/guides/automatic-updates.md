# Automatic host updates

From 0.1.12, automatic installation defaults to **on**, and can be disabled in Settings → System → Software Update or with `removent-cli settings set update_auto_install false`. An existing `update_check_enabled = false` choice is respected. Switching installation off leaves manual updates and interrupted-install recovery available.

Installed applications register a user LaunchAgent backed by an independent signed recovery executable in `userdata/update-agent`. The host and main window may both be closed or broken: the recovery agent continues to read the installed version and check the selected signed release feed. Automatic checks run at most every six hours; the foreground app retains its existing manual check and 24-hour check schedule. The installed launcher and packaged CLI service-start flow bootstrap the agent. Development and staging copies do not register it automatically.

## When installation happens

Close the main window and finish remote connections. Native connections, TLS handshakes, VNC, outgoing desktop connections and local sync operations participate in an admission gate. A second gate covers the same application opened with different data directories. The installer takes exclusive ownership before replacement; new connections are refused until installation or recovery finishes. Persistent markers keep admission closed if the installer dies.

The download is checked against the signed manifest, SHA-256, release version, channel and every component's Developer ID identity. Before stopping the host, the agent verifies ownership, writable installation space, signatures, permission declarations, entitlements and protocol metadata; copies and flushes the candidate on the destination volume; then starts the actual desktop with disposable data. Its smoke mode must render a frame, remain alive for ten seconds, and exit cleanly. It does not start discovery, CloudKit, hosting, the tray or permission prompts.

Changed permissions, protocol compatibility, a read-only installation, a different owner, a legacy host without admission support, or an unmanaged development host defer the installation. Fix the reported issue or use an interactive installation. Settings corruption fails closed; it never silently enables automatic installation.

## Restart and recovery

A durable journal is written before quiescing the host and tray. Same-volume renames retain `Removent.app.update-backup`. The replacement host must answer versioned IPC, retain its identity and existing screen/input grants, report its listener ready when sharing is enabled, and keep the same process ID throughout 60 seconds of observation. An explicitly stopped host stays stopped; its executable is checked using disposable data instead. No unattended TCC or administrator prompt is requested.

An incomplete transaction or failed startup restores the verified previous bundle and restarts the user-session host according to the existing stop intent. The backup is verified before restoration, and remains available after success until a later healthy replacement. A failed release is recorded in `userdata/update-agent/rejected.json`; higher repair releases remain eligible. Settings writes are blocked during maintenance, and the daemon reads settings strictly. Recovery commands have deadlines and do not require temporary output files on a full disk. Failure to record the final rollback journal does not prevent restarting an already restored, verified host.

The pinned supervisor does not live inside the replaceable application. It runs signed, immutable updater workers, observes their startup and heartbeat, and falls back when a worker crashes or stalls. A new worker is staged after the application passes its health checks. Worker promotion retains the prior worker; obsolete worker files are retired. The pinned executable remains the final recovery fallback. Future releases must preserve the journal, worker protocol, startup receipt and health-response compatibility or provide an explicitly reviewed migration.

## macOS permission boundaries

The administrator-managed **login-window host has its own root-owned application copy**. Unattended user-session updates leave that working copy intact. General settings and the background update status report when it needs an administrator-approved update. A change to the application protocol is deferred, so this split does not silently create incompatible login-window access.

The updater never runs as root, exports Keychain credentials, edits TCC databases or accepts an administrator password. Developer ID, component bundle IDs and existing permission declarations are retained. A missing permission or macOS background-item restriction is reported; it is not bypassed. Initial migration from 0.1.11 uses that version's existing installer, so install 0.1.12 before relying on the new independent recovery mechanism.

Inspect independent status, even when the desktop is unavailable:

```sh
/Applications/Removent.app/Contents/MacOS/removent-update --status
# If the application is temporarily absent, use the pinned copy:
"$HOME/Library/Application Support/removent/userdata/update-agent/removent-update" --status
```

Custom server installations outside `/Applications` or `~/Applications` can explicitly register the signed helper with `removent-update --install /path/to/Removent.app`, retaining the same `REMOVENT_DATA_DIR` used by that host. The updater's LaunchAgent belongs to the logged-in user, and macOS must allow its background execution.

See [local acceptance evidence](../validation/automatic-update-2026-11-07.md). Startup checks and bounded observation reduce failure risk; they do not prove every remote-control, capture, audio or network path remains bug-free. The independent update channel is retained for those later repairs.

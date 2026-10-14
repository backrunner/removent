# Wake-on-LAN validation (2026-10-14)

Desktop target context menus now offer a manual wake request and configuration.
Saved target details expose the same actions through More. Configuration is
local to the controller, supports a validated MAC, IPv4 destination and UDP port,
and survives ordinary connection edits without touching Keychain credentials.
Changing the destination clears the previous target's configuration.

## Automated checks

The full-package results below were captured before concurrent system-account
authentication changes. The final desktop rebuild and focused Wake-on-LAN UI
test also passed after those changes.

- `cargo test --locked -p removent-client --lib -- --test-threads=1`: 104 passed, 2 ignored.
- `cargo test --locked -p removent-app -- --test-threads=1`: 100 passed, 2 ignored.
- `cargo clippy --locked -p removent-client -p removent-app --all-targets -- -D warnings`: passed. Cargo reports the existing `block 0.1.6` future compatibility warning.
- Wake-on-LAN source formatting and `git diff --check`: passed. Package-wide
  formatting passed before subsequent unrelated authentication edits.
- `scripts/check_layout.py` passed during the initial feature checks. A later
  run after concurrent changes failed because `packages/host/src/session/handshake.rs`
  had 659 lines against its 650-line budget. Wake-on-LAN does not edit that file.
- Actual UDP loopback reception verifies the six `FF` bytes and sixteen exact MAC repetitions, totalling 102 bytes.
- Bookmark tests cover old JSON compatibility, credential preservation, concurrent renames, removal, destination changes and atomic discovery configuration.
- Cloud sync excludes wake settings, retains local settings on metadata edits, and removes them when the remote destination changes.
- GPUI interaction coverage right-clicks a different target from the current selection, checks the correct target, validates the form at 860×600, and verifies that invalid Save/Enter keeps the form open.

## Native macOS checks

An isolated app bundle and data directory were used. Sharing, discovery, tray
startup and update checks were disabled for the test instance.

The real desktop window was exercised in dark and light themes:

1. Right-click target and open configuration.
2. Confirm an empty MAC with Enter: useful validation error, form remains open.
3. Enter `02:11:22:33:44:55`, `127.0.0.1`, port `43999`; save with Enter.
4. Right-click Send Wake-on-LAN; a separate Python UDP receiver receives the packet.
5. Restart the app in light theme; reopen and inspect persisted settings.
6. Remove wake settings; the target bookmark remains available.

Receiver result:

```json
{"length":102,"header":"ffffffffffff","mac":"021122334455","valid":true}
```

The UI states only that the request was sent; it does not claim the target woke.
Actual sleeping-Mac wake, Wi-Fi wake, subnet broadcast delivery and wake via a
LAN gateway were not tested. Gateway forwarding is not implemented. No production
app installation, release or remote CI run was performed for this change.
Unrelated concurrent working-tree changes were retained.

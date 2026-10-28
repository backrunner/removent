# Clipboard sharing validation (2026-10-29)

## Findings and changes

RVP already carries `ClipboardSync` and `ClipboardAck` in both directions. The
desktop controller and daemon both bridge the system text clipboard, but their
250 ms background pollers do not send content that predates a connection. Input
also travels independently of clipboard snapshots, so Command+V could reach the
host before the new text and paste the host's previous content.

The native client now captures a stable text snapshot on Command+V, sends it
through a bounded reliable clipboard queue, and waits for its matching ack before
forwarding the paste key and subsequent input. The control pump continues serving
responses and disconnects. The wait has a five-second deadline; failure closes the
connection so host teardown releases held input. Clipboard opt-out preserves the
ordinary remote paste shortcut.

Forced snapshots include text copied before connecting and explicit empty text.
Unavailable or oversized text cannot trigger a stale paste. A delayed background
snapshot cannot replace a newer forced snapshot. Remote writes and local polling
share a lock; suppression uses the actual change count and checks that a concurrent
local copy has not replaced the incoming text. A host text-write failure ends the
session before acknowledging, rather than allowing a paste of old content.

The wire format and protocol version are unchanged. Current sharing is UTF-8 plain
text, up to 4 MiB, including Chinese, multiple lines and emoji. RTF, HTML, PNG and
file references have protocol tags but no end-to-end bridge. The iOS clipboard
panel provides explicit send/copy actions; it does not automatically synchronize
the system pasteboard. LoginWindow hosting does not offer clipboard bridging.
`ClipboardAck` remains a receipt ack for compatibility; a peer without clipboard
capability or a local bridge does not thereby confirm a successful system write.

## Evidence

- Core, network, client and host regression run: 322 passed, 5 ignored, one
  explicitly filtered pre-existing failure. This includes the nine new native
  client clipboard tests and three additional core clipboard regressions.
- Local QUIC tests exercise both production control pumps and both
  pollers using in-memory clipboards. They verify clipboard contents at the input
  injection boundary, key-down/up ordering after the matching ack, immediate
  copy/paste, pre-connection Unicode text, empty text, capability opt-out, no echo,
  stale snapshot rejection, invalid/oversized content, write failure and timeout.
- The network regression also transfers a 4 MiB snapshot while ensuring unrelated
  key releases remain deliverable on the control stream.
- Desktop Viewer tests: 11 passed, including a new GPUI event-dispatch test for
  Command+V key-down/up with the physical V key code and no Unicode insertion.
- Final workspace Clippy, all targets with `-D warnings`: passed. Cargo reports
  the existing `block 0.1.6` dependency future-compatibility notice.
- Formatting, layout/source-size checks and `git diff --check`: passed.

The desktop test and final Clippy build initially ran out of disk space on the
workspace volume. Only compiler caches created during this task were removed.
The successful final runs disabled incremental compilation and redirected new
compiler outputs to the system volume using a temporary compiler wrapper; they
used the same source, dependency artifacts and test assertions. No application
data or unrelated project caches were removed. Temporary compiler artifacts and
links were removed after verification.

The unfiltered host run fails
`session::audio::tests::audio_capture_eof_does_not_cancel_video` at the five-second
video-header wait (`packages/host/src/session/audio/tests.rs:73`). The same isolated
test fails on the unchanged base commit `18a04d25b5e021dfa543e7a078485c5cb66fb6c9`
in a temporary detached worktree. This is a reproducible baseline failure, not a
passing full-suite result. The temporary worktree was removed afterward.

Reproduce the passing regression scope with:

```sh
rtk cargo test --locked -p removent-core -p removent-net -p removent-client -p removent-host --no-fail-fast -- --test-threads=1 --skip session::audio::tests::audio_capture_eof_does_not_cancel_video
rtk cargo test --locked -p removent-app --bin removent ui::viewer::tests -- --test-threads=1
rtk cargo clippy --locked --workspace --all-targets -- -D warnings
```

These results do not establish two-machine system-pasteboard/AppKit acceptance,
WAN behavior or physical iOS device acceptance. The changes have not been
installed into the current application/daemon or released. Source delivery is
recorded in Git history.

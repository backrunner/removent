# Implementation review — second pass (2026-09-07)

This pass examined native session termination, media task ownership, negotiation
waits, clipboard capability enforcement, persistence and adaptive quality recovery.
It preserves the existing working tree and the first pass's fixes.

## Fixes

1. **Native session termination:** a received `SessionEnd`, control EOF or control
   failure now closes the connection and aborts both the media dispatcher and its
   owned media tasks. Clipboard polling stops too. Previously the control task
   could exit while frame/PCM receivers remained open indefinitely, freezing the
   viewer and preventing normal session completion. `JoinSet` ownership also
   removes the race where a dispatcher registered a child after teardown had
   already drained the shared task list.
2. **Graceful client close:** `close()` waits for the control pump to send
   `SessionEnd`, finish the send stream and allow QUIC delivery acknowledgement
   before teardown. Closing is bounded to two seconds; a stalled peer cannot
   block it indefinitely. Previously enqueueing the end message was immediately
   followed by aborting the task that was supposed to send it.
3. **Negotiation timeout:** client and host wait against a fixed deadline for each
   expected message. Unrelated messages no longer reset the timeout and keep an
   incomplete negotiation alive indefinitely.
4. **Clipboard opt-out and formats:** a disabled clipboard capability prevents
   polling and application on the client, and prevents host polling in both full
   negotiation and quick resume. Both control pumps only apply `TextUtf8` to their
   text clipboard bridges; HTML/RTF/other payloads are acknowledged without being
   misinterpreted as text.
5. **Atomic settings/trust writes:** every save uses an exclusively created,
   randomly named sibling file with mode `0600`, then syncs and renames it. Failed
   writes clean up that file. Concurrent writers cannot rename each other's shared
   `.tmp` file or modify a file another writer has already published.
6. **Persistence failures:** settings/trust reads only treat `NotFound` as an empty
   store. Other read failures are returned. Corrupt settings only fall back after
   a successful backup. Trust upsert/removal commits in-memory changes after a
   successful save, so a failed write does not silently change active trust.
7. **Adaptive recovery:** healthy samples stop degradation immediately and start
   the existing five-second recovery window. A ten-minute outage no longer needs
   ten minutes of good samples merely to cancel accumulated bad samples, and good
   samples cannot trigger further downgrade. Bad-window tracking is bounded to
   the three levels used by the downgrade policy.
8. **Adaptive bitrate ceiling:** manual presets cap the negotiated bitrate instead
   of raising that ceiling. For example, a 2 Mbps session with a 12 Mbps preset
   cannot grow beyond the session's 2 Mbps negotiation during healthy recovery.

## Regression coverage

Added real QUIC tests for pending media shutdown on `SessionEnd`/EOF, delivery of
the outbound close message, unrelated-traffic negotiation deadlines, clipboard
opt-out and unsupported formats. Extended the full-session fixture to check that
a host does not start a clipboard poller when the client opts out.

Persistence tests run eight simultaneous writers with repeated 64 KiB payloads,
check complete visible files and permissions, force rename/read/backup failures,
and verify unchanged in-memory trust on failed persistence. Adaptation tests
cover a ten-minute outage, healthy samples after a burst of bad samples, and all
manual presets with a lower negotiated bitrate.

- `cargo test --workspace --no-fail-fast -- --test-threads=1`: **215 passed,
  1 failed**. The failure is the existing mDNS self-discovery test
  (`discovery::tests::advertise_and_browse_loopback`), also seen before this pass.
  All new regressions and RDP/VNC/native-session/media integration tests passed.
- `cargo fmt --all --check` and `git diff --check`: passed.
- After the final service-side deadline adjustment, the native session suite
  passed all eight tests (including pairing, cancellation, clipboard opt-out,
  media and quick resume).
- `cargo clippy --workspace --all-targets -- -D warnings`: passed on the final
  implementation. Only the existing dependency future-compatibility notices for
  `block` and `proc-macro-error2` remain.

## Remaining scope

The production adaptation pipeline still needs actual periodic telemetry and
transport-pressure integration, as recorded in the first pass. This pass fixes
controller behavior; it does not claim that end-to-end adaptation is complete.
Atomic replacement guarantees complete files, not merging separate stale settings
snapshots. Real two-machine/TCC/constrained-network testing remains outstanding.

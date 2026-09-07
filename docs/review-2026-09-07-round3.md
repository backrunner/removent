# Implementation review — third pass (2026-09-07)

This pass focuses on stalled connections, temporary interruptions, retries,
resource ownership and media backlogs. It preserves the pre-existing working tree
and the preceding review fixes.

## Fixed findings

1. **Host stop left negotiation tasks alive.** The runner now owns connection,
   handshake, busy-rejection and VNC listener tasks in a `JoinSet`. Cancellation
   reaches pairing/admission before a session has been established. Handshakes
   no longer serialize acceptance of other peers; the task count is bounded at
   16. Stopping closes the endpoint and cancels/drains the owned tasks, with an
   abort fallback. Session child handles abort on drop, including early returns.
   Session-end callbacks and the busy bit also have drop-based cleanup.
2. **Blocked native writes prevented cancellation and held input releases.**
   Host control/video/audio cancellation now surrounds the entire asynchronous
   operation, including writes. Native control and media writes have separate
   ten-second deadlines: QUIC keepalives cannot keep a blocked application write
   alive indefinitely. The host's input tracker releases keys/buttons on normal
   exit, cancellation and task abort. Host `SessionEnd` finishes its control
   stream and briefly waits for delivery before teardown.
3. **VNC write errors skipped input cleanup.** VNC input release now uses a drop
   guard, so `?` on a failed frame write and task abort both release held keys.
   Cancellation can interrupt an in-progress write; writes also have a ten-second
   deadline. Listener/client forwarding and reader tasks have explicit ownership.
4. **Video backpressure accounting was ineffective.** The serial writer called
   submit/write/sent in order, so its `SendGate` backlog never grew enough to
   trigger degradation. Production now measures actual write duration; writes
   taking at least 100 ms signal pressure to the controller. Pressure and remote
   stats use the same monotonic elapsed-time accounting, fixing the zero-duration
   pressure samples that never advanced the controller's hysteresis clock.
5. **Slow media paths retained stale captures.** ScreenCaptureKit now publishes
   video into a single latest-frame slot, shared by the native and VNC paths.
   The final capture replaces an older unread one even when the screen becomes
   static afterward. Frames are discarded before encoding; encoded delta frames
   retain their reference chain. Encoder dedup bookkeeping retains at most eight
   submitted pixel buffers. Raw audio backlog is coalesced before Opus encoding.
6. **Input could accidentally cancel reconnection.** A closed old command channel
   no longer reports queue overload. The bridge pauses input publication while
   reconnecting, preserves the viewer channel and checks the connection generation
   before publishing a replacement. Explicit `SessionEnd` is recorded separately
   from the subsequent local QUIC close, keeping unexpected stream EOF retryable.
7. **Retries could hold the old session and wait minutes.** The bridge drops the
   old session before reconnecting, including when media failed but control stayed
   alive and still occupied the host slot. Each of the three quick-resume attempts
   is capped at eight seconds, with 300 ms between attempts; fallback admission
   cannot extend an individual attempt to the normal interactive wait. Audio
   forwarding aborts on drop and is stopped/joined at interruption; queued playback
   is cleared before retry. Audio device startup runs off the Tokio worker thread.
8. **Incomplete media startup could freeze the viewer indefinitely.** Accepting
   the negotiated media streams and their type bytes now has a ten-second total
   startup deadline. Once a video/audio frame has started, reads of its remaining
   header and payload each have a ten-second deadline. Waiting for the first byte
   of a subsequent frame remains unbounded so static screens are not disconnected
   merely for having no visual changes.

## Validation

Seven new regression tests cover:

- real QUIC control write starvation, cancellation, task abort and the ten-second
  write timeout, with held-key release assertions;
- real QUIC audio write starvation followed by cancellation;
- stopping a host while an unknown peer has completed the control handshake but
  never supplies its pairing stream, including a resource-retention assertion;
- VNC stalled/erroring writes using a bounded duplex transport, including input
  release on cancellation, abort and I/O error;
- coalescing a raw backlog while accepting the next capture;
- paused/replaced input channels and rejection of stale connection generations;
- missing media streams closing the viewer despite a live control connection.

Existing regressions now distinguish clean end from unexpected EOF, verify that
video-only sessions receive the final capture rather than requiring an arbitrary
number of intermediate frames, and feed adaptation samples at their real cadence.

Final checks:

- `cargo test --workspace --no-fail-fast -- --test-threads=1`: **222 passed,
  1 failed**. All seven new regressions and native-session/VNC/RDP tests passed.
  The remaining failure is the pre-existing
  `discovery::tests::advertise_and_browse_loopback` self-discovery failure.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all --check` and `git diff --check`: passed.

Earlier runs also encountered intermittent local TCP `AddrNotAvailable`
(`Can't assign requested address`, macOS error 49) in VNC/RDP fixtures. Those
fixtures passed in the final full run. No host networking settings or unrelated
processes were changed to obtain that result. Existing dependency future-
compatibility notices for `block` and `proc-macro-error2` remain.

Logs: `/tmp/removent-round3-final.log` and
`/tmp/removent-round3-clippy-final.log`.

## Verification limits

These checks establish bounded queues, application I/O deadlines and cancellation
behavior under synthetic stalls; they do not measure real two-machine latency,
throughput, battery use or extended Wi-Fi outage recovery. Synchronous macOS
capture/codec calls are not preemptible by Tokio cancellation and still require
hardware/TCC testing. Transport buffers can retain bytes already accepted for
sending; a latest-frame slot cannot retract those bytes.

Production write-pressure degradation is now connected. Periodic client telemetry,
automatic quality recovery from local healthy-write samples, and applying dynamic
FPS/scale decisions remain incomplete, as noted in earlier reviews. This pass does
not claim that the full adaptive-quality feature is complete.

Follow-up: dynamic FPS/scaling and production adaptive recovery were subsequently
implemented and tested; see [adaptive-quality-2026-09-07.md](adaptive-quality-2026-09-07.md).

# Dynamic FPS, scaling and adaptive recovery

Implemented on 2026-09-07, following the third implementation review.

## Runtime behavior

The native RVP control pump now samples media delivery every 250 ms. Measurements
include completed writes, writes stalled for at least 100 ms, and deltas of QUIC
sent/lost packets. Fresh peer StatsReport messages can contribute additional bad
network evidence; local congestion is never overridden by a healthy peer report.
Missing or stale measurements interrupt the healthy recovery window.

The existing downgrade policy now reaches the video pipeline in full:

- Reduce bitrate toward 1 Mbps.
- On continued congestion, halve FPS down to 15 FPS (never increase a negotiated
  ceiling that is already below 15).
- Reduce linear resolution scale through 75% and 50%, retaining aspect ratio and
  even dimensions for the codecs.

Changes remain separated by at least two seconds. After five seconds of sustained
healthy delivery, restore one dimension per recovery window: scale first, then
FPS, then bitrate in 25% increments up to the negotiated ceiling. Recovery no
longer increases all three dimensions together.

A watch channel carries the entire QualityState to the video loop. It retains the
latest decision during a blocked write instead of losing updates when a FIFO is
full. Controllers are initialized from the actual negotiated bitrate/FPS, including
quick resume, rather than the host configuration's potentially higher values.

## Applying FPS and scale

The sender paces actual encoder submissions using monotonic deadlines. Deadlines
start from completion, so a stalled connection cannot release a burst of queued
frames to catch up. Raw captures remain in the existing latest-frame slot.

ScreenCaptureKit retains the base capture dimensions. Accelerate/vImage resamples
BGRA before encoding; retaining the original capture also permits recovery of a
static screen's full detail. FPS changes affect encoder submission cadence and
encoder configuration; they do not reconfigure ScreenCaptureKit's callback rate.

Changing dimensions or FPS rebuilds the encoder. The next frame is a keyframe
with CONFIG_CHANGED, carrying the new dimensions and decoder configuration. Cached
screen content is refreshed even when no new capture arrives. Short runs of
encoder errors or delayed output retry the cached frame with a bounded attempt
count; persistent failure ends the session instead of silently freezing it.

While quality is reduced and no recent frame has been sent, the sender refreshes
the cached screen once per second to obtain real delivery measurements. These
probes allow static screens to recover sharpness. Probes stop at full quality.
A single successful write expires as health evidence after 1.5 seconds, so it
cannot by itself satisfy the five-second recovery requirement.

## Input and protocol compatibility

FrameGeometry { width, height } is appended as control variant 26 (27 variants in
total). Existing variant indices are unchanged; older receivers skip the new
message using the existing unknown-variant framing rule.

The viewer publishes its displayed dimensions on the same ordered control FIFO
as mouse events, and republishes them after resume. The host changes input scaling
when that message arrives, rather than when the encoder produces a new size.
Thus queued events for an old frame keep their old coordinate interpretation.
Cached mouse positions and host teardown positions are rescaled as geometry
changes, preserving button-release locations during a drag or focus loss.

For an input-enabled client, the host enables adaptive scaling only after receiving
valid FrameGeometry. Older clients retain bitrate/FPS adaptation without dynamic
scaling. View-only sessions can scale immediately. Library users displaying native
frames should send FrameGeometry before mouse events for a newly displayed size.
VNC/RDP compatibility paths ignore this native control extension.

## Validation

Nine additional tests cover:

- Actual wire cadence under a capture flood after switching from 60 to 15 FPS.
- Cached-frame downscale and upscale, CONFIG_CHANGED/keyframes, monotonic timestamps
  and decoding at each size for H.264, HEVC and AV1.
- Control-pump recovery driven by actual static refresh writes, including a 50% →
  75% wire-size change after the healthy interval and its QualityControl message.
- Geometry and mouse-event ordering across shrink/restore transitions.
- Correct held-button release position after resizing.
- Stale delivery evidence, in-progress stalls and fresh successful writes.
- Interrupted recovery windows and conservative scaling for legacy input clients.
- Accelerate resizing with constant BGRA colors and malformed input rejection.
- The appended geometry variant's wire index and framing roundtrip.

Existing client lifecycle tests also verify geometry cache invalidation on resume.

Final checks:

- `cargo test --workspace --no-fail-fast -- --test-threads=1`: **231 passed,
  1 failed**. The failure is the previously observed
  `discovery::tests::advertise_and_browse_loopback` self-discovery failure.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all --check` and `git diff --check`: passed.

Logs: `/tmp/removent-adapt-final.log` and
`/tmp/removent-adapt-clippy-final.log`.

These tests use real local QUIC connections and codecs, plus synthetic captures;
they do not measure two-machine Wi-Fi throughput, long-duration outage recovery,
or total capture/encode CPU use on other hardware. Periodic client decode/render
telemetry is not required by this implementation: adaptation has its own host-side
write/transport measurements. Further network-specific tuning can use those
measurements without changing the quality-delivery or resize protocol.

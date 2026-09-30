use super::*;

/// Resident processing pump for the control stream: Pong replies, KeyframeRequest
/// forwarding (rate-limited), StatsReport → adaptation controller → QualityControl delivery.
pub struct ControlPumpDeps {
    pub conn: RvpConnection,
    /// Keyframe injection endpoint for the video loop (None when there is no video loop).
    pub kf_tx: Option<mpsc::Sender<()>>,
    pub controller: Option<std::sync::Arc<std::sync::Mutex<AdaptationController>>>,
    pub window_ms: u64,
    pub input: Option<Arc<dyn InputSink>>,
    pub local_clip: Option<Arc<dyn TextClipboard>>,
    pub quality_tx: Option<tokio::sync::watch::Sender<QualityState>>,
    /// Peer's negotiated capability set: without input/clipboard, the corresponding
    /// messages are ignored.
    pub caps: Caps,
    /// Session-level clipboard state (shares the suppress singleton with the poller to
    /// prevent echo loops).
    pub clip_state: Option<Arc<ClipSyncState>>,
    /// Session-level stop token: cancelled when the pump exits, bringing down the media loops.
    pub cancel: CancellationToken,
    /// Peer fingerprint hex: on pump exit (= session end) the resume token's
    /// validity window is re-anchored to this moment (protocol.md §7.3).
    pub peer_fp: Option<String>,
    /// Actual write progress, stalls and QUIC loss sampled by the controller.
    pub delivery: Option<Arc<crate::delivery::DeliveryHealth>>,
}

/// Minimum interval between KeyframeRequests (protocol.md §7.1).
pub(super) const KEYFRAME_MIN_INTERVAL: Duration = Duration::from_millis(500);

/// Per-session input injection rate limit (events/second, burst = 1s worth).
pub(super) const INPUT_RATE_LIMIT_PER_SEC: f64 = 600.0;

/// Simple token bucket for rate limiting high-frequency input events.
pub struct TokenBucket {
    pub(super) tokens: f64,
    pub(super) capacity: f64,
    pub(super) refill_per_sec: f64,
    pub(super) last: std::time::Instant,
}

impl TokenBucket {
    pub fn new(rate_per_sec: f64) -> Self {
        Self {
            tokens: rate_per_sec,
            capacity: rate_per_sec,
            refill_per_sec: rate_per_sec,
            last: std::time::Instant::now(),
        }
    }

    pub fn try_take(&mut self) -> bool {
        self.try_take_at(std::time::Instant::now())
    }

    pub(super) fn try_take_at(&mut self, now: std::time::Instant) -> bool {
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

pub(super) fn sample_elapsed_ms(last: &mut std::time::Instant) -> u64 {
    let now = std::time::Instant::now();
    let elapsed = now.duration_since(*last).as_millis().min(u64::MAX as u128) as u64;
    *last = now;
    elapsed
}

/// Input cleanup must also run when the owning service aborts this task.
pub(super) struct ControlCleanup {
    pub(super) tracker: crate::input_sink::InputReleaseTracker,
    pub(super) input: Option<Arc<dyn InputSink>>,
    pub(super) cancel: CancellationToken,
    pub(super) peer_fp: Option<String>,
}

impl Drop for ControlCleanup {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(input) = &self.input {
            self.tracker.release_all(input.as_ref());
        }
        if let Some(fp) = &self.peer_fp {
            refresh_resume(fp);
        }
    }
}

/// Control pump: inbound handling (Pong/keyframe/stats adaptation/input
/// injection/clipboard application) plus outbound command forwarding. The only task
/// holding the sink for the duration of the session.
pub fn spawn_control_pump(
    mut source: ControlSource,
    sink: ControlSink,
    deps: ControlPumpDeps,
    mut cmd_rx: mpsc::Receiver<ControlMsg>,
) -> tokio::task::JoinHandle<()> {
    let mut cleanup = ControlCleanup {
        tracker: Default::default(),
        input: deps.input.clone(),
        cancel: deps.cancel.clone(),
        peer_fp: deps.peer_fp.clone(),
    };
    tokio::spawn(async move {
        let cancel = deps.cancel.clone();
        let (mut clipboard, clipboard_tx) =
            removent_net::clipboard::ClipboardReader::new(deps.conn.clone());
        let mut writer =
            removent_net::control_writer::ControlWriter::with_clipboard(sink, clipboard_tx);
        let work = async {
            let mut last_kf: Option<std::time::Instant> = None;
            let mut input_bucket = TokenBucket::new(INPUT_RATE_LIMIT_PER_SEC);
            let mut remote_sample: Option<(std::time::Instant, Sample)> = None;
            let mut input_geometry: Option<(u32, u32)> = None;
            let mut sample_tick = tokio::time::interval(Duration::from_millis(250));
            sample_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Injection failures (e.g. Accessibility permission revoked mid-session)
            // are logged at most once per 5s to avoid flooding at 600 events/s.
            let mut last_inject_warn: Option<std::time::Instant> = None;
            let mut note_inject_err = |what: &str, e: String| {
                if last_inject_warn.is_none_or(|t| t.elapsed() >= Duration::from_secs(5)) {
                    tracing::warn!(%what, err=%e, "input injection failed (accessibility permission revoked?)");
                    last_inject_warn = Some(std::time::Instant::now());
                }
            };
            // Both network reports and local write-pressure samples share one
            // monotonic clock. Event-only samples must advance hysteresis too.
            let mut last_sample_at = std::time::Instant::now();
            // Fallback suppress when there is no shared clip_state (does not prevent echo
            // loops, only makes writes possible).
            let local_suppress = std::sync::atomic::AtomicU64::new(0);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    result = writer.progress(), if writer.has_pending() => {
                        if !matches!(result, Ok(false)) { break; }
                    }
                    item = clipboard.next(&mut source) => {
                        let Some(item) = item else { break };
                        let Ok(item) = item else { break };
                        match item {
                            ControlItem::Msg(m) => match *m {
                                ControlMsg::Ping { ts_us } => {
                                    if writer.enqueue(ControlMsg::Pong { ts_us }).is_err() {
                                        break;
                                    }
                                }
                                ControlMsg::KeyframeRequest => {
                                    // Rate limit: minimum interval 500ms (protocol.md §7.1).
                                    let now = std::time::Instant::now();
                                    if last_kf.is_none_or(|t| now.duration_since(t) >= KEYFRAME_MIN_INTERVAL) {
                                        last_kf = Some(now);
                                        if let Some(kf) = deps.kf_tx.as_ref() {
                                            let _ = kf.try_send(());
                                        }
                                    }
                                }
                                ControlMsg::StatsReport {
                                    rtt_ms,
                                    loss_pct,
                                    recv_kbps,
                                    jitter_ms,
                                    decode_ms,
                                    ..
                                } => {
                                    let sample = Sample { rtt_ms, loss_pct, recv_kbps, jitter_ms, decode_ms };
                                    if sample.is_valid() {
                                        remote_sample = Some((std::time::Instant::now(), sample));
                                    }
                                }
                                ControlMsg::FrameGeometry { width, height } => {
                                    if (2..=MAX_CAPTURE_W).contains(&width) && (2..=MAX_CAPTURE_H).contains(&height) {
                                        if let Some(old) = input_geometry.replace((width, height)) {
                                            cleanup.tracker.rescale_position(old, (width, height));
                                        }
                                        if let Some(input) = &deps.input { input.set_capture_dims(width, height); }
                                    }
                                }
                                ControlMsg::MouseEvent { display_id, x_px, y_px, buttons, kind } => {
                                    if !deps.caps.input {
                                        tracing::warn!("mouse event from client without input cap, ignored");
                                        continue;
                                    }
                                    // Release events bypass the rate limiter: dropping
                                    // a button-up would leave the button stuck on the
                                    // host until session teardown.
                                    let bypass = matches!(
                                        kind,
                                        MouseKind::LeftUp | MouseKind::RightUp | MouseKind::MiddleUp
                                    );
                                    if !bypass && !input_bucket.try_take() {
                                        tracing::warn!("input rate limit exceeded, dropping mouse event");
                                        continue;
                                    }
                                    cleanup.tracker.note_mouse(display_id, x_px, y_px, kind);
                                    if let Some(input) = deps.input.as_ref()
                                        && let Err(e) = input.mouse(display_id, x_px, y_px, buttons, kind)
                                    {
                                        note_inject_err("mouse", e);
                                    }
                                }
                                ControlMsg::KeyEvent { vk_code, modifiers, kind, unicode } => {
                                    if !deps.caps.input {
                                        tracing::warn!("key event from client without input cap, ignored");
                                        continue;
                                    }
                                    // Key releases and modifier state changes bypass
                                    // the rate limiter (stuck keys otherwise).
                                    let bypass = matches!(kind, KeyKind::Up | KeyKind::FlagsChanged);
                                    if !bypass && !input_bucket.try_take() {
                                        tracing::warn!("input rate limit exceeded, dropping key event");
                                        continue;
                                    }
                                    cleanup.tracker.note_key(vk_code, modifiers, kind);
                                    if let Some(input) = deps.input.as_ref()
                                        && let Err(e) = input.key(vk_code, modifiers, kind, unicode)
                                    {
                                        note_inject_err("key", e);
                                    }
                                }
                                ControlMsg::ScrollEvent { display_id, dx_mm, dy_mm, phase } => {
                                    if !deps.caps.input {
                                        tracing::warn!("scroll event from client without input cap, ignored");
                                        continue;
                                    }
                                    // Scroll-end phases bypass the rate limiter so a
                                    // gesture never lingers in "scrolling" state.
                                    if phase != ScrollPhase::Ended && !input_bucket.try_take() {
                                        tracing::warn!("input rate limit exceeded, dropping scroll event");
                                        continue;
                                    }
                                    if let Some(input) = deps.input.as_ref()
                                        && let Err(e) = input.scroll(display_id, dx_mm, dy_mm, phase)
                                    {
                                        note_inject_err("scroll", e);
                                    }
                                }
                                ControlMsg::ClipboardSync { seq, format, data } => {
                                    if !deps.caps.clipboard {
                                        tracing::warn!("clipboard sync from client without clipboard cap, ignored");
                                        // Ack receipt even when this peer did not negotiate
                                        // clipboard application; otherwise a sender waiting
                                        // for confirmation can stall indefinitely.
                                        if writer.enqueue(ControlMsg::ClipboardAck { seq }).is_err() {
                                            break;
                                        }
                                        continue;
                                    }
                                    if format == removent_proto::ClipFormat::TextUtf8
                                        && let Some(clip) = deps.local_clip.as_ref() {
                                        // Shares the suppress singleton with the poller: after writing remote
                                        // content, the poller skips echoing it back.
                                        let suppress: &std::sync::atomic::AtomicU64 = deps
                                            .clip_state
                                            .as_ref()
                                            .map(|s| &s.suppress_cc)
                                            .unwrap_or(&local_suppress);
                                        if let Err(e) = removent_core::apply_incoming_clip(
                                            clip.as_ref(),
                                            suppress,
                                            &data,
                                        ) {
                                            tracing::warn!(err=%e, "clipboard apply failed");
                                        }
                                    }
                                    // Receipt is acknowledged independently of whether a local
                                    // clipboard implementation exists.
                                    if writer.enqueue(ControlMsg::ClipboardAck { seq }).is_err() {
                                        break;
                                    }
                                }
                                ControlMsg::SessionEnd { .. } => break,
                                _ => {}
                            },
                            ControlItem::Skipped => continue,
                        }
                    }
                    maybe_cmd = cmd_rx.recv(), if writer.accepts_commands() => {
                        match maybe_cmd {
                            Some(msg @ ControlMsg::SessionEnd { .. }) => {
                                // Outbound SessionEnd must actually reach the peer before we exit.
                                if writer.enqueue(msg).is_err() { break; }
                            }
                            None => break,
                            Some(msg) => {
                                if writer.enqueue(msg).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    _ = sample_tick.tick() => {
                        let elapsed = sample_elapsed_ms(&mut last_sample_at);
                        let local = deps.delivery.as_ref().and_then(|health| health.sample());
                        let remote = remote_sample.as_ref().filter(|(at, _)| at.elapsed() <= Duration::from_millis(750)).map(|(_, sample)| *sample);
                        let sample = match (local, remote) {
                            (Some(healthy), remote) => {
                                let mut sample = remote.unwrap_or(Sample { rtt_ms: 0., loss_pct: 0., recv_kbps: 0, jitter_ms: 0., decode_ms: 0. });
                                if !healthy { sample.jitter_ms = sample.jitter_ms.max(100.); }
                                Some(sample)
                            }
                            (None, remote) => remote,
                        };
                        let decision = deps.controller.as_ref().and_then(|controller| {
                            let mut controller = controller.lock().unwrap();
                            let before = controller.state();
                            if let Some((width, height)) = deps.delivery.as_ref().and_then(|d| d.capture_dims()) {
                                controller.set_dimensions(width, height);
                            }
                            if elapsed > 1000 || sample.is_none() { controller.pause_recovery(); }
                            if let Some(sample) = sample { controller.on_sample(&sample, elapsed.min(1000)); }
                            if let Some(delivery) = &deps.delivery {
                                delivery.update_send_budget(controller.state().bitrate_kbps);
                            }
                            (before != controller.state()).then(|| controller.state())
                        });
                        if let Some(st) = decision {
                            if let Some(tx) = &deps.quality_tx { tx.send_replace(st); }
                            if writer.enqueue(ControlMsg::QualityControl {
                                bitrate_kbps: st.bitrate_kbps, fps: st.fps, scale: st.scale,
                            }).is_err() { break; }
                        }
                    }
                }
            }
        };
        // A send can block on QUIC flow control even while keepalives succeed.
        // Cancel the entire operation, including an in-progress sink.send.
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {},
            _ = work => {},
        }
        drop(cleanup);
    })
}

//! Per-connection negotiation, media tasks and session cleanup.

use super::*;
use crate::session::{
    ControlPumpDeps, HostConfig, HostInteractions, serve_connection_with_display, spawn_audio_loop,
    spawn_control_pump, spawn_video_loop,
};

/// Resets the mDNS busy bit when the session task exits (any path).
struct BusyGuard(Arc<Advertiser>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        if let Err(e) = self.0.set_busy(false) {
            tracing::warn!(err=%e, "mDNS busy-bit reset failed");
        }
    }
}

/// Dropping a JoinHandle detaches it; session children must instead be aborted.
#[derive(Default)]
struct SessionTasks(Vec<tokio::task::JoinHandle<()>>);

impl Drop for SessionTasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

struct SessionExit {
    cancel: CancellationToken,
    active: Arc<std::sync::Mutex<Option<CancellationToken>>>,
}

impl Drop for SessionExit {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.active.lock().unwrap().take();
    }
}

struct PairingDisplayGuard {
    shown: Arc<AtomicBool>,
    cbs: Arc<HostCallbacks>,
}

impl Drop for PairingDisplayGuard {
    fn drop(&mut self) {
        if self.shown.swap(false, Ordering::SeqCst) {
            (self.cbs.on_event)(HostEvent::PairingCleared);
        }
    }
}

/// Serves one accepted connection: negotiation/pairing, then the media session.
/// Holds the session semaphore permit until the session ends.
pub(super) async fn run_connection(
    ctx: ConnectionCtx,
    conn: RvpConnection,
    _permit: tokio::sync::OwnedSemaphorePermit,
) {
    // Reload peers per connection: pairing/grant changes take effect immediately.
    let mut peers = match PeersStore::load(&ctx.paths) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(err=%e, "peers store load failed");
            return;
        }
    };
    let peer_fp = conn.peer_fingerprint().map(hex::encode).unwrap_or_default();
    if ctx.settings.paired_only && !peers.by_fingerprint(&peer_fp).is_some_and(|p| p.trusted) {
        conn.inner().close(1u32.into(), b"paired devices only");
        return;
    }

    let pairing_shown = Arc::new(AtomicBool::new(false));
    let _pairing_display = PairingDisplayGuard {
        shown: pairing_shown.clone(),
        cbs: ctx.cbs.clone(),
    };
    let shown_pin = pairing_shown.clone();
    let cb_pin = ctx.cbs.clone();
    let cb_adm = ctx.cbs.clone();
    let interactions = HostInteractions {
        show_pairing_pin: Box::new(move |pin| {
            shown_pin.store(true, Ordering::SeqCst);
            (cb_pin.show_pairing_pin)(pin);
        }),
        admission_prompt: Box::new(move |peer_name, fp16| {
            (cb_adm.admission_prompt)(peer_name, fp16)
        }),
    };

    let host_cfg = HostConfig {
        authentication: ctx.settings.authentication.clone(),
        auth_paths: Some(ctx.paths.clone()),
        audio_available: !ctx.settings.window_server_capture,
        preapproved_only: ctx.settings.paired_only,
        device_name: ctx.settings.device_name.clone(),
        admission: ctx.settings.admission,
        video_bitrate_kbps: removent_proto::MAX_VIDEO_BITRATE_KBPS,
        video_fps: removent_proto::MAX_VIDEO_FPS,
        input_sink: ctx.input_sink.clone(),
        local_clip: if ctx.settings.window_server_capture {
            None
        } else {
            ctx.local_clip.clone()
        },
    };

    let mut display_awake = None;
    let mut capture_display = None;
    let served = serve_connection_with_display(
        conn.clone(),
        &ctx.identity,
        &mut peers,
        &host_cfg,
        interactions,
        || async {
            // Authentication and admission precede this callback, including
            // quick resume. Unapproved peers cannot wake the physical screen.
            display_awake = Some(super::display::DisplayAwake::new());
            let display = super::display::wait_for_display().await?;
            capture_display = Some(display.clone());
            Ok(display)
        },
    )
    .await;

    let (established, kf_rx, quality_rx, cmd_rx, sink, source) = match served {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(err=%e, "connection closed before session established");
            // Give a terminal protocol error time to reach the client before
            // the owning task closes QUIC (which otherwise races the error).
            let _ = tokio::time::timeout(Duration::from_secs(1), conn.inner().closed()).await;
            return;
        }
    };
    let display = capture_display.expect("established session has a prepared display");

    pairing_shown.store(false, Ordering::SeqCst);
    let peer_name = peers
        .by_fingerprint(&established.peer_fp_hex)
        .map(|p| p.name.clone())
        .unwrap_or_else(|| established.peer_name.clone());
    if host_cfg.authentication.mode == removent_core::AuthenticationMode::PairingCode {
        (ctx.cbs.on_event)(HostEvent::PairingDone {
            peer_name: peer_name.clone(),
        });
    }
    let _session_events = SessionEventGuard::new(
        ctx.cbs.clone(),
        peer_name,
        established.peer_fp_hex.chars().take(16).collect(),
        format!("{:?}", established.ack.video.codec),
    );

    // Advertise the busy bit while the session lives (reset on any exit path).
    if let Err(e) = ctx.advertiser.set_busy(true) {
        tracing::warn!(err=%e, "mDNS busy-bit set failed");
    }
    let _busy_guard = BusyGuard(ctx.advertiser.clone());
    // Expose the session stop token for the graceful-shutdown path.
    *ctx.active_session.lock().unwrap() = Some(established.cancel.clone());
    let _exit = SessionExit {
        cancel: established.cancel.clone(),
        active: ctx.active_session.clone(),
    };
    let mut tasks = SessionTasks::default();

    // Warn once if input injection would silently fail (Accessibility TCC).
    if host_cfg.input_sink.is_some() && !removent_input::accessibility_trusted() {
        tracing::warn!(
            "accessibility permission not granted; input injection will fail (System Settings > Privacy & Security > Accessibility)"
        );
    }

    // The control pump samples actual delivery and owns the complete quality state.
    let delivery = Arc::new(crate::delivery::DeliveryHealth::new(conn.clone()));

    // Control pump: input injection / clipboard application / adaptive delivery
    // (trimmed by the peer's capabilities).
    let deps = ControlPumpDeps {
        conn: conn.clone(),
        kf_tx: Some(established.keyframe_req_tx.clone()),
        controller: Some(established.controller.clone()),
        window_ms: 250,
        input: host_cfg.input_sink.clone(),
        local_clip: host_cfg.local_clip.clone(),
        quality_tx: Some(established.quality_tx.clone()),
        caps: established.peer_caps,
        clip_state: established.clip_state.clone(),
        cancel: established.cancel.clone(),
        peer_fp: Some(established.peer_fp_hex.clone()),
        delivery: Some(delivery.clone()),
    };
    tasks.0.push(spawn_control_pump(source, sink, deps, cmd_rx));

    // Capture at the connection ceiling; adaptation scales encoded frames within its range.
    let (video_tx, video_rx) =
        removent_core::latest::channel::<removent_media_capture::CapturedVideoFrame>();
    let (audio_tx, audio_rx) = tokio::sync::mpsc::channel::<removent_media_capture::AudioFrame>(32);
    let (cap_w, cap_h) = established
        .video_quality
        .unwrap_or_default()
        .fit_dimensions(display.w_px, display.h_px);
    let (w, h) = (cap_w as usize, cap_h as usize);
    // Tell the input sink which frame dimensions the peer's coordinates refer
    // to, so it can rescale capture px → display physical px before injection.
    if let Some(sink) = host_cfg.input_sink.as_ref() {
        sink.set_capture_dims(cap_w, cap_h);
    }
    let vstream = match conn.open_media_stream().await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(err=%e, "open video stream failed");
            established.cancel.cancel();
            return;
        }
    };
    tasks.0.push(spawn_video_loop(
        vstream,
        video_rx,
        kf_rx,
        quality_rx,
        established.ack.video.codec,
        w,
        h,
        established.ack.video.max_bitrate_kbps,
        established.ack.video.max_fps,
        established.cancel.clone(),
        // Fatal encoder errors end the session explicitly (client is notified).
        Some(established.cmd_tx.clone()),
        display.id,
        Some(delivery),
        host_cfg.input_sink.clone(),
    ));
    // Audio follows the negotiation result: a peer that declined audio gets no
    // audio stream, no encode loop, and no audio capture (§5.2).
    let audio_enabled = established.ack.audio.enabled;
    if audio_enabled {
        let astream = match conn.open_media_stream().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(err=%e, "open audio stream failed");
                established.cancel.cancel();
                return;
            }
        };
        tasks.0.push(spawn_audio_loop(
            astream,
            audio_rx,
            established.ack.audio.bitrate_kbps,
            established.cancel.clone(),
        ));
    }

    if let Err(error) = super::capture::run(
        display,
        ctx.settings.window_server_capture,
        established.video_quality.unwrap_or_default(),
        established.ack.video.max_fps,
        // Retain the producers until an explicit failure reaches the peer;
        // otherwise media EOF can cancel control before SessionEnd is sent.
        video_tx.clone(),
        audio_enabled.then_some(audio_tx.clone()),
        host_cfg.input_sink.clone(),
        established.cancel.clone(),
    )
    .await
    {
        tracing::error!(%error, "capture recovery exhausted");
        let _ = established
            .cmd_tx
            .send(ControlMsg::SessionEnd {
                reason: removent_proto::EndReason::InternalError,
            })
            .await;
        let _ = tokio::time::timeout(Duration::from_secs(1), established.cancel.cancelled()).await;
        established.cancel.cancel();
    }
    // Release input before returning the session permit. Drop handles safely
    // on early return or cancellation as well.
    tasks.0[0].abort();
    let _ = (&mut tasks.0[0]).await;
}
#[cfg(test)]
mod tests;

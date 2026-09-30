use super::*;

pub(super) fn mark_submitted_frame_sent(
    dedup: &mut FrameDeduplicator,
    submitted_frames: &mut HashMap<i64, Arc<[u8]>>,
    pts_us: i64,
) {
    if let Some(sent_bgra) = submitted_frames.remove(&pts_us) {
        dedup.mark_sent_shared(sent_bgra);
    }
}

/// Video send loop: latest raw capture → exact deduplication → encode →
/// write to the media uni-stream.
/// Returns the JoinHandle; the loop exits immediately when `cancel` fires
/// (control-pump exit / session end). `fatal_tx` receives a SessionEnd when the
/// encoder fails fatally (init at session start or rebuild after a resolution
/// change), so the client gets an explicit end instead of a silent video hang.
/// `capture_display_id` identifies the captured display for resolution-change
/// rebuilds; `delivery` records write progress and stalls for the
/// control pump (which routes them through the adaptation controller).
/// Input geometry is acknowledged by the viewer on the control stream, so
/// encoding a new size cannot change the interpretation of queued old input.
#[allow(clippy::too_many_arguments)]
pub fn spawn_video_loop(
    mut stream: removent_net::quinn::SendStream,
    mut video_rx: removent_core::latest::Receiver<(Vec<u8>, i64)>,
    mut keyframe_req_rx: mpsc::Receiver<()>,
    mut quality_rx: tokio::sync::watch::Receiver<QualityState>,
    codec: CodecId,
    width: usize,
    height: usize,
    bitrate_kbps: u32,
    fps: u8,
    cancel: CancellationToken,
    fatal_tx: Option<mpsc::Sender<ControlMsg>>,
    capture_display_id: u64,
    delivery: Option<Arc<crate::delivery::DeliveryHealth>>,
    _input: Option<Arc<dyn InputSink>>,
) -> tokio::task::JoinHandle<()> {
    let cancel_on_exit = cancel.clone().drop_guard();
    tokio::spawn(async move {
        let _cancel_on_exit = cancel_on_exit;
        let work = async {
            let mut ceiling = AdaptationController::with_dimensions(
                bitrate_kbps,
                fps,
                removent_core::QualityPreset::Auto,
                width as u32,
                height as u32,
            )
            .state();
            let mut quality = bounded_quality(*quality_rx.borrow_and_update(), ceiling);
            let mut source_dims = (width, height);
            if let Some(health) = &delivery {
                health.set_capture_dims(width as u32, height as u32);
            }
            let mut dims = scaled_dims(source_dims, quality.scale);
            let mut encoder =
                VideoEncoder::new(codec, dims.0, dims.1, quality.bitrate_kbps, quality.fps)
                    .map_err(|e| e.to_string())?;
            let mut quality_open = true;
            let mut capture_open = true;
            let mut next_frame = tokio::time::Instant::now();
            let mut last_raw: Option<(Arc<[u8]>, i64)> = None;
            let mut pending = false;
            let mut last_sent = tokio::time::Instant::now();
            let mut force = false;
            let mut refresh_unchanged = false;
            let mut config_changed = false;
            let mut frame_id = 0;
            let mut failed_encodes = 0;
            let mut last_pts: Option<i64> = None;
            let mut dedup = FrameDeduplicator::new();
            let mut submitted = HashMap::new();
            loop {
                if !capture_open && !pending {
                    break;
                }
                tokio::select! {
                    _ = tokio::time::sleep_until(last_sent + Duration::from_secs(1)), if quality != ceiling && last_raw.is_some() && !pending => {
                        // Probe only while degraded. Reuse temporal references:
                        // forcing a large I-frame every second can itself congest
                        // a WAN link. Quality changes and explicit requests still
                        // force keyframes to restore detail / reset the decoder.
                        pending = true;
                        refresh_unchanged = true;
                    }
                    changed = quality_rx.changed(), if quality_open => {
                        if changed.is_err() { quality_open = false; continue; }
                        let new = bounded_quality(*quality_rx.borrow_and_update(), ceiling);
                        if new == quality { continue; }
                        let new_dims = scaled_dims(source_dims, new.scale);
                        if new_dims != dims || new.fps != quality.fps {
                            // Replace only after successful construction; the first
                            // packet of this configuration must be independently decodable.
                            let replacement = VideoEncoder::new(codec, new_dims.0, new_dims.1, new.bitrate_kbps, new.fps)
                                .map_err(|e| e.to_string())?;
                            encoder = replacement;
                            dims = new_dims;
                            config_changed = true;
                        } else {
                            encoder.set_bitrate_kbps(new.bitrate_kbps).map_err(|e| e.to_string())?;
                        }
                        quality = new;
                        dedup.reset();
                        submitted.clear();
                        force = true;
                        pending = last_raw.is_some();
                        next_frame = tokio::time::Instant::now();
                    }
                    Some(()) = keyframe_req_rx.recv() => {
                        force = true;
                        pending = last_raw.is_some();
                    }
                    item = video_rx.recv(), if capture_open => {
                        let Some((bgra, pts)) = item else { capture_open = false; continue };
                        if bgra.len() != source_dims.0 * source_dims.1 * 4 {
                            let Some(new_dims) = current_capture_dims(capture_display_id)
                                .filter(|(w, h)| w * h * 4 == bgra.len()) else {
                                tracing::warn!("capture dimensions unavailable; skipping mismatched frame");
                                continue;
                            };
                            source_dims = new_dims;
                            if let Some(health) = &delivery { health.set_capture_dims(new_dims.0 as u32, new_dims.1 as u32); }
                            ceiling = AdaptationController::with_dimensions(
                                bitrate_kbps, fps, removent_core::QualityPreset::Auto, new_dims.0 as u32, new_dims.1 as u32,
                            ).state();
                            dims = scaled_dims(source_dims, quality.scale);
                            encoder = VideoEncoder::new(codec, dims.0, dims.1, quality.bitrate_kbps, quality.fps)
                                .map_err(|e| e.to_string())?;
                            config_changed = true;
                            force = true;
                            dedup.reset();
                            submitted.clear();
                        }
                        last_raw = Some((bgra.into(), pts));
                        pending = true;
                    }
                    _ = tokio::time::sleep_until(next_frame), if pending => {
                        pending = false;
                        let (raw, pts) = last_raw.as_ref().expect("pending capture");
                        let bgra: Arc<[u8]> = if dims == source_dims { raw.clone() } else {
                            removent_media_codec::scale::scale_bgra(raw, source_dims, dims)?.into()
                        };
                        if !dedup.should_encode(&bgra, force || refresh_unchanged) {
                            next_frame = tokio::time::Instant::now() + frame_interval(quality.fps);
                            continue;
                        }
                        if force { encoder.request_keyframe(); }
                        let pts = last_pts.map_or(*pts, |last| (*pts).max(last.saturating_add(1)));
                        last_pts = Some(pts);
                        submitted.insert(pts, bgra);
                        if submitted.len() > 8 && let Some(oldest) = submitted.keys().copied().min() {
                            submitted.remove(&oldest);
                        }
                        let frames = match encoder.encode_bgra(submitted.get(&pts).unwrap(), pts) {
                            Ok(frames) => frames,
                            Err(e) => {
                                tracing::warn!(err=%e, "video encode failed; forcing refresh");
                                dedup.reset();
                                submitted.clear();
                                force = true;
                                pending = true;
                                failed_encodes += 1;
                                if failed_encodes >= 8 { return Err(format!("encoder repeatedly failed: {e}")); }
                                next_frame = tokio::time::Instant::now() + frame_interval(quality.fps);
                                continue;
                            }
                        };
                        let mut sent = false;
                        for ef in frames {
                            frame_id += 1;
                            let mut flags = if ef.keyframe { removent_proto::video_flags::KEYFRAME } else { 0 };
                            if ef.keyframe && config_changed {
                                flags |= removent_proto::video_flags::CONFIG_CHANGED;
                                config_changed = false;
                            }
                            let hdr = removent_proto::VideoFrameHeader {
                                frame_id, pts_us: ef.pts_us, flags, codec,
                                width: dims.0 as u16, height: dims.1 as u16,
                                payload_len: annexb_len_of(&ef),
                            };
                            let wire = build_video_frame(&hdr, &ef.data);
                            if let Some(health) = &delivery { health.begin_write(); }
                            let result = tokio::time::timeout(Duration::from_secs(10), removent_net::session::write_media(&mut stream, &wire)).await;
                            if let Some(health) = &delivery { health.end_write(); }
                            if !matches!(result, Ok(Ok(()))) { return Ok::<(), String>(()); }
                            sent = true;
                            mark_submitted_frame_sent(&mut dedup, &mut submitted, ef.pts_us);
                        }
                        if sent {
                            failed_encodes = 0;
                            force = false;
                            refresh_unchanged = false;
                            last_sent = tokio::time::Instant::now();
                        } else {
                            // A quality-limited VT encoder may intentionally
                            // drop frames. Let adaptation reduce the frame rate;
                            // retain the cached refresh, with a real time bound.
                            pending = true;
                            if let Some(health) = &delivery { health.encoder_limited(); }
                            if last_sent.elapsed() >= Duration::from_secs(10) {
                                return Err("encoder produced no frames for ten seconds".into());
                            }
                        }
                        if codec != CodecId::Av1 { submitted.remove(&pts); }
                        // Schedule from completion: a stalled writer must never
                        // burst to catch up with elapsed frame deadlines.
                        next_frame = tokio::time::Instant::now() + frame_interval(quality.fps);
                    }
                }
            }
            Ok::<(), String>(())
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {},
            result = work => {
                if let Err(e) = result {
                    tracing::error!(err=%e, "video pipeline failed");
                    if let Some(tx) = fatal_tx {
                        let _ = tokio::time::timeout(Duration::from_secs(1), tx.send(ControlMsg::SessionEnd {
                            reason: removent_proto::EndReason::InternalError,
                        })).await;
                        let _ = tokio::time::timeout(Duration::from_secs(1), cancel.cancelled()).await;
                    }
                }
            },
        }
    })
}

pub(super) fn bounded_quality(value: QualityState, ceiling: QualityState) -> QualityState {
    QualityState {
        bitrate_kbps: value.bitrate_kbps.clamp(1, ceiling.bitrate_kbps.max(1)),
        fps: value.fps.clamp(1, ceiling.fps.max(1)),
        scale: if value.scale.is_finite() {
            value.scale.clamp(0.5, 1.0)
        } else {
            1.0
        },
    }
}

pub(super) fn scaled_dims((w, h): (usize, usize), scale: f32) -> (usize, usize) {
    (
        ((w as f32 * scale) as usize & !1).max(2),
        ((h as f32 * scale) as usize & !1).max(2),
    )
}

pub(super) fn frame_interval(fps: u8) -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(fps.max(1)))
}

/// Coalesce raw audio captures. Snapshot the queue length so a fast producer
/// cannot keep this drain running indefinitely.
pub(super) fn newest_queued<T>(mut newest: T, rx: &mut mpsc::Receiver<T>) -> T {
    for _ in 0..rx.len() {
        match rx.try_recv() {
            Ok(frame) => newest = frame,
            Err(_) => break,
        }
    }
    newest
}

/// Capture bounding box: the encoder input never exceeds 1920×1080.
pub const MAX_CAPTURE_W: u32 = 1920;
pub const MAX_CAPTURE_H: u32 = 1080;

/// Fits display pixel dims into the capture bounding box, preserving the
/// display's aspect ratio (an independent per-axis clamp would stretch e.g.
/// 16:10 panels). Never upscales; results are forced even for the encoder.
pub fn fit_capture_dims(w_px: u32, h_px: u32) -> (u32, u32) {
    let s = (MAX_CAPTURE_W as f64 / w_px as f64)
        .min(MAX_CAPTURE_H as f64 / h_px as f64)
        .min(1.0);
    let w = ((w_px as f64 * s) as u32) & !1;
    let h = ((h_px as f64 * s) as u32) & !1;
    (w.max(2), h.max(2))
}

/// Current capture dimensions for `display_id`, mirroring the runner's sizing
/// rule (the captured display's pixels aspect-fit into 1920×1080). Falls back
/// to the first display when the id is unknown; None in headless environments.
pub(super) fn current_capture_dims(display_id: u64) -> Option<(usize, usize)> {
    let list = removent_input::display_list();
    let d = list.iter().find(|d| d.id == display_id).or(list.first())?;
    let (w, h) = fit_capture_dims(d.w_px, d.h_px);
    Some((w as usize, h as usize))
}

pub(super) fn annexb_len_of(ef: &removent_media_codec::EncodedVideoFrame) -> u32 {
    ef.data.len() as u32
}

use super::*;
use removent_media_capture::{CapturedPixels, CapturedVideoFrame};

pub(super) fn mark_submitted_frame_sent(
    dedup: &mut FrameDeduplicator,
    submitted_frames: &mut HashMap<i64, Arc<Vec<u8>>>,
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
    mut video_rx: removent_core::latest::Receiver<removent_media_capture::CapturedVideoFrame>,
    mut keyframe_req_rx: mpsc::Receiver<()>,
    mut quality_rx: tokio::sync::watch::Receiver<QualityState>,
    codec: CodecId,
    width: usize,
    height: usize,
    bitrate_kbps: u32,
    fps: u8,
    cancel: CancellationToken,
    fatal_tx: Option<mpsc::Sender<ControlMsg>>,
    _capture_display_id: u64,
    delivery: Option<Arc<crate::delivery::DeliveryHealth>>,
    _input: Option<Arc<dyn InputSink>>,
) -> tokio::task::JoinHandle<()> {
    let cancel_on_exit = cancel.clone().drop_guard();
    tokio::spawn(async move {
        let _cancel_on_exit = cancel_on_exit;
        let work = async {
            // The controller owns the pixel/frame budget. Reapplying a full-size
            // readability floor here would cap FPS even after it reduced pixels.
            let ceiling = QualityState {
                bitrate_kbps,
                fps,
                scale: 1.,
            };
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
            let mut last_raw: Option<Arc<CapturedVideoFrame>> = None;
            let mut native_available = codec != CodecId::Av1;
            let mut last_native_sent = None;
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
            let mut capture_generation = None;
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
                        let Some(frame) = item else { capture_open = false; continue };
                        let new_dims = (frame.width, frame.height);
                        if frame.width == 0 || frame.height == 0 || frame.width.max(frame.height) > MAX_CAPTURE_W as usize || frame.width.min(frame.height) > MAX_CAPTURE_H as usize
                            || !frame.pixels.valid_for(frame.width, frame.height) {
                            tracing::warn!("invalid capture dimensions; skipping frame");
                            continue;
                        }
                        if capture_generation.replace(frame.generation) != Some(frame.generation) {
                            force = true;
                            config_changed = true;
                            dedup.reset();
                            submitted.clear();
                        }
                        if new_dims != source_dims {
                            source_dims = new_dims;
                            if let Some(health) = &delivery { health.set_capture_dims(new_dims.0 as u32, new_dims.1 as u32); }
                            dims = scaled_dims(source_dims, quality.scale);
                            encoder = VideoEncoder::new(codec, dims.0, dims.1, quality.bitrate_kbps, quality.fps)
                                .map_err(|e| e.to_string())?;
                            config_changed = true;
                            force = true;
                            dedup.reset();
                            submitted.clear();
                        }
                        last_raw = Some(Arc::new(frame));
                        pending = true;
                    }
                    _ = tokio::time::sleep_until(next_frame), if pending => {
                        pending = false;
                        let encode_started = std::time::Instant::now();
                        let raw = last_raw.as_ref().expect("pending capture");
                        let native = matches!(raw.pixels, CapturedPixels::Native(_)) && native_available;
                        let mut cpu = None;
                        let should_encode = if native {
                            force || refresh_unchanged || last_native_sent != Some((raw.generation, raw.pts_us))
                        } else {
                            let pixels = cpu_pixels(raw, dims)?;
                            let changed = dedup.should_encode(&pixels, force || refresh_unchanged);
                            cpu = Some(pixels);
                            changed
                        };
                        if !should_encode {
                            next_frame = tokio::time::Instant::now() + frame_interval(quality.fps);
                            continue;
                        }
                        if force { encoder.request_keyframe(); }
                        let pts = last_pts.map_or(raw.pts_us, |last| raw.pts_us.max(last.saturating_add(1)));
                        last_pts = Some(pts);
                        let result = if native {
                            let CapturedPixels::Native(buffer) = &raw.pixels else { unreachable!() };
                            match encoder.encode_native(buffer, pts) {
                                Err(removent_media_codec::VideoError::Gpu(error)) => {
                                    tracing::warn!(%error, "GPU frame processing unavailable; using CPU fallback");
                                    native_available = false;
                                    let pixels = cpu_pixels(raw, dims)?;
                                    let result = encoder.encode_bgra(&pixels, pts);
                                    submitted.insert(pts, pixels);
                                    result
                                }
                                result => result,
                            }
                        } else {
                            let pixels = cpu.expect("CPU pixels prepared");
                            let result = encoder.encode_bgra(&pixels, pts);
                            submitted.insert(pts, pixels);
                            result
                        };
                        if submitted.len() > 8 && let Some(oldest) = submitted.keys().copied().min() {
                            submitted.remove(&oldest);
                        }
                        let frames = match result {
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
                        if let Some(health) = &delivery {
                            health.encoded(encode_started.elapsed(), frame_interval(quality.fps));
                        }
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
                            if native { last_native_sent = Some((raw.generation, raw.pts_us)); }
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
                        // Encode/write time consumes this frame's budget. Do not
                        // add another full interval or accumulate missed deadlines.
                        next_frame = next_frame_deadline(next_frame, tokio::time::Instant::now(), quality.fps);
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
            value.scale.clamp(0.125, 1.0)
        } else {
            1.0
        },
    }
}

pub(super) fn scaled_dims((w, h): (usize, usize), scale: f32) -> (usize, usize) {
    (
        ((w as f32 * scale).round() as usize & !1).max(2),
        ((h as f32 * scale).round() as usize & !1).max(2),
    )
}

pub(super) fn frame_interval(fps: u8) -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(fps.max(1)))
}

pub(super) fn next_frame_deadline(
    scheduled: tokio::time::Instant,
    completed: tokio::time::Instant,
    fps: u8,
) -> tokio::time::Instant {
    (scheduled + frame_interval(fps)).max(completed)
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

/// Absolute long/short-edge bounds; each connection selects its own ceiling.
pub const MAX_CAPTURE_W: u32 = 3840;
pub const MAX_CAPTURE_H: u32 = 2160;

/// Fits display pixel dims into the capture bounding box, preserving the
/// display's aspect ratio (an independent per-axis clamp would stretch e.g.
/// 16:10 panels). Never upscales; results are forced even for the encoder.
pub fn fit_capture_dims(w_px: u32, h_px: u32) -> (u32, u32) {
    removent_proto::VideoQuality::Ultra.fit_dimensions(w_px, h_px)
}

pub(super) fn annexb_len_of(ef: &removent_media_codec::EncodedVideoFrame) -> u32 {
    ef.data.len() as u32
}

fn cpu_pixels(frame: &CapturedVideoFrame, dims: (usize, usize)) -> Result<Arc<Vec<u8>>, String> {
    if dims == (frame.width, frame.height)
        && let CapturedPixels::Bgra(pixels) = &frame.pixels
    {
        return Ok(pixels.clone());
    }
    let source = frame.pixels.bgra(frame.width, frame.height)?;
    let pixels = if dims == (frame.width, frame.height) {
        source.into_owned()
    } else {
        removent_media_codec::scale::scale_bgra(&source, (frame.width, frame.height), dims)?
    };
    Ok(Arc::new(pixels))
}

#[cfg(test)]
mod pacing_tests {
    use super::*;
    #[test]
    fn processing_consumes_the_frame_budget_and_stalls_do_not_accumulate_debt() {
        let start = tokio::time::Instant::now();
        let frame = frame_interval(60);
        assert_eq!(
            next_frame_deadline(start, start + Duration::from_millis(8), 60),
            start + frame
        );
        let stalled = start + Duration::from_millis(100);
        assert_eq!(next_frame_deadline(start, stalled, 60), stalled);
        assert_eq!(
            next_frame_deadline(stalled, stalled + Duration::from_millis(8), 60),
            stalled + frame
        );
    }
}

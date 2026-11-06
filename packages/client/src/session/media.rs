use super::*;

/// Accept media uni-streams and dispatch by the first-byte stream_type to the
/// video/audio loops.
/// Stream count follows the negotiation result: when audio.enabled=false only the
/// video stream is expected (§5.2).
/// The dispatch task owns its sub-loops, so dropping it also aborts them.
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_media_loops(
    conn: RvpConnection,
    bgra_tx: removent_core::latest::Sender<DecodedFrame>,
    pcm_tx: mpsc::Sender<crate::audio::DecodedAudio>,
    audio_enabled: bool,
    native_video: bool,
    cmd_tx: mpsc::Sender<ControlMsg>,
    last_kf: Arc<Mutex<Option<Instant>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(media_dispatch_loop(
        conn,
        bgra_tx,
        pcm_tx,
        audio_enabled,
        native_video,
        cmd_tx,
        last_kf,
    ))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn media_dispatch_loop(
    conn: RvpConnection,
    bgra_tx: removent_core::latest::Sender<DecodedFrame>,
    pcm_tx: mpsc::Sender<crate::audio::DecodedAudio>,
    audio_enabled: bool,
    native_video: bool,
    cmd_tx: mpsc::Sender<ControlMsg>,
    last_kf: Arc<Mutex<Option<Instant>>>,
) {
    use removent_proto::{STREAM_TYPE_AUDIO, STREAM_TYPE_VIDEO};
    let mut media = tokio::task::JoinSet::new();
    let feedback = Arc::new(VideoFeedback::default());
    let _reporter = AbortOnDrop(tokio::spawn(crate::video_feedback::report_loop(
        conn.clone(),
        feedback.clone(),
        cmd_tx.clone(),
    )));
    let mut headers = tokio::task::JoinSet::new();
    let mut video_tx = Some(bgra_tx);
    let mut audio_tx = audio_enabled.then_some(pcm_tx);
    let video_deadline = tokio::time::sleep(Duration::from_secs(10));
    tokio::pin!(video_deadline);
    // Audio can be silent or unavailable even when negotiated. Its absence or
    // EOF must never tear down an already running video stream. Keep accepting
    // late audio, but bound header tasks from malformed/duplicate streams.
    let mut accepted = 0;
    loop {
        tokio::select! {
            _ = &mut video_deadline, if video_tx.is_some() => {
                tracing::warn!("video stream failed to start within deadline");
                return;
            }
            stream = conn.accept_media_stream(), if accepted < 8 && (video_tx.is_some() || audio_tx.is_some()) => {
                let Ok(mut stream) = stream else { return };
                accepted += 1;
                headers.spawn(async move {
                    let mut first = [0u8; 1];
                    stream.read_exact(&mut first).await.map(|_| (first[0], stream))
                });
            }
            header = headers.join_next(), if !headers.is_empty() => {
                match header {
                    Some(Ok(Ok((STREAM_TYPE_VIDEO, stream)))) if video_tx.is_some() => {
                        let tx = video_tx.take().unwrap();
                        let cmd_tx = cmd_tx.clone();
                        let last_kf = last_kf.clone();
                        let feedback = feedback.clone();
                        media.spawn(async move {
                            video_recv_loop(stream, tx, cmd_tx, last_kf, feedback, native_video).await;
                            STREAM_TYPE_VIDEO
                        });
                    }
                    Some(Ok(Ok((STREAM_TYPE_AUDIO, stream)))) if audio_tx.is_some() => {
                        let tx = audio_tx.take().unwrap();
                        media.spawn(async move {
                            audio_recv_loop(stream, tx).await;
                            STREAM_TYPE_AUDIO
                        });
                    }
                    Some(Ok(Ok((kind, _)))) => tracing::warn!(kind, "unexpected or duplicate media stream"),
                    _ => tracing::warn!("media stream header failed"),
                }
            }
            ended = media.join_next(), if !media.is_empty() => {
                match ended {
                    Some(Ok(STREAM_TYPE_AUDIO)) => tracing::warn!("audio stream ended; retaining video"),
                    // Video EOF/failure closes the frame channel promptly, even
                    // when an optional audio stream/header is still pending.
                    _ => return,
                }
            }
        }
    }
}

/// Rebuild the decoder on demand (hot) from the frame header: on CONFIG_CHANGED or a
/// resolution/codec change, rebuild using the keyframe's inline parameter sets
/// (protocol.md §6.1).
pub(super) fn rebuild_decoder(
    hdr: &removent_proto::VideoFrameHeader,
    payload: &[u8],
    tx: removent_core::latest::Sender<DecodedFrame>,
    feedback: Arc<VideoFeedback>,
    native_video: bool,
) -> Option<VideoDecoder> {
    let ps = if hdr.codec == removent_proto::CodecId::Av1 {
        Vec::new()
    } else {
        extract_param_sets(payload, hdr.codec == removent_proto::CodecId::Hevc)
    };
    let (width, height) = (u32::from(hdr.width), u32::from(hdr.height));
    match VideoDecoder::with_surface_output(
        hdr.codec,
        width as usize,
        height as usize,
        &ps,
        native_video,
        move |frame| {
            feedback.decoded(frame.pts_us);
            let _ = tx.send(DecodedFrame {
                surface: frame.surface,
                data: frame.data,
                width,
                height,
                pts_us: frame.pts_us,
            });
        },
    ) {
        Ok(d) => Some(d),
        Err(e) => {
            tracing::error!(err=%e, "decoder init failed");
            None
        }
    }
}

/// Dropping a JoinHandle does not stop the task; wrap it in abort-on-drop to prevent leaks.
pub(super) struct AbortOnDrop(pub(super) tokio::task::JoinHandle<()>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Sanity bound on one compressed video payload: `payload_len` is an unchecked u32 from
/// the wire, and an absurd value must not force a multi-GiB allocation (a real keyframe
/// is orders of magnitude smaller).
pub(super) const MAX_VIDEO_PAYLOAD_BYTES: u32 = 64 * 1024 * 1024;

/// Reject an absurd payload length before allocating; the stream is desynced/corrupt at
/// that point, so the caller drops it.
pub(super) fn payload_len_ok(len: u32) -> bool {
    if len > MAX_VIDEO_PAYLOAD_BYTES {
        tracing::error!(
            payload_len = len,
            "video payload exceeds sanity bound, dropping stream"
        );
        return false;
    }
    true
}

/// Enqueue a KeyframeRequest, rate-limited to one per KEYFRAME_MIN_INTERVAL. The limiter
/// state is shared with [`ClientSession::request_keyframe`] so every request path (app,
/// first-frame, config-change, decode failure) honours the same 500ms minimum interval
/// (protocol.md §7.1).
pub(super) fn request_keyframe(
    cmd_tx: &mpsc::Sender<ControlMsg>,
    last_kf: &Mutex<Option<Instant>>,
) {
    {
        let mut last = last_kf.lock().unwrap();
        let now = Instant::now();
        if last.is_some_and(|t| now.duration_since(t) < KEYFRAME_MIN_INTERVAL) {
            return;
        }
        *last = Some(now);
    }
    let _ = cmd_tx.try_send(ControlMsg::KeyframeRequest);
}

/// Once a frame has started, partial headers/payloads cannot wait forever
/// while transport keepalives continue. Quiet static screens may still wait
/// indefinitely for the first byte of the next frame.
pub(super) async fn read_media_part(
    stream: &mut removent_net::quinn::RecvStream,
    bytes: &mut [u8],
) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_secs(10), stream.read_exact(bytes)).await,
        Ok(Ok(()))
    )
}

pub(super) async fn video_recv_loop(
    mut stream: removent_net::quinn::RecvStream,
    bgra_tx: removent_core::latest::Sender<DecodedFrame>,
    cmd_tx: mpsc::Sender<ControlMsg>,
    last_kf: Arc<Mutex<Option<Instant>>>,
    feedback: Arc<VideoFeedback>,
    native_video: bool,
) {
    use removent_proto::video_flags;
    feedback.begin_frame();
    // The first byte was already consumed; the remaining header is 26 bytes.
    let mut rest = [0u8; 26];
    if !read_media_part(&mut stream, &mut rest).await {
        return;
    }
    let mut head_bytes = [0u8; 27];
    head_bytes[0] = removent_proto::STREAM_TYPE_VIDEO;
    head_bytes[1..].copy_from_slice(&rest);
    let Ok((hdr0, _)) = parse_video_header(&head_bytes) else {
        tracing::error!("first video header parse failed");
        return;
    };

    // Decoder callbacks publish directly into the latest-frame slot. Static
    // screens no longer wake a drain task 500 times per second.
    let mut decoder: Option<VideoDecoder> = None;
    // Current decoder configuration (codec, w, h), used to detect when a hot rebuild
    // is needed.
    let mut cur_cfg: Option<(removent_proto::CodecId, u16, u16)> = None;

    // First frame (should be a keyframe): read its payload and initialize the decoder.
    if !payload_len_ok(hdr0.payload_len) {
        return;
    }
    let mut first_payload = vec![0u8; hdr0.payload_len as usize];
    if !read_media_part(&mut stream, &mut first_payload).await {
        return;
    }
    feedback.received(hdr0.pts_us, first_payload.len() + 27);
    if hdr0.is_keyframe() {
        match rebuild_decoder(
            &hdr0,
            &first_payload,
            bgra_tx.clone(),
            feedback.clone(),
            native_video,
        ) {
            Some(d) => {
                feedback.decoding(hdr0.pts_us);
                if let Err(e) = d.decode_annexb(&first_payload, hdr0.pts_us) {
                    feedback.decode_failed(hdr0.pts_us);
                    // First-frame decode failure: nudge the host for a fresh keyframe
                    // (rate-limited, shared limiter with the other request paths)
                    // instead of only logging and waiting for the periodic IDR.
                    tracing::warn!(err=%e, "first frame decode failed, requesting keyframe");
                    request_keyframe(&cmd_tx, &last_kf);
                }
                decoder = Some(d);
                cur_cfg = Some((hdr0.codec, hdr0.width, hdr0.height));
            }
            None => {
                // First-frame init failed: request a new keyframe to retry instead of
                // exiting silently.
                tracing::error!("first frame decoder init failed, requesting keyframe");
                request_keyframe(&cmd_tx, &last_kf);
            }
        }
    } else {
        tracing::warn!("first video frame is not a keyframe, requesting keyframe");
        request_keyframe(&cmd_tx, &last_kf);
    }

    loop {
        // The renderer is gone: stop reading (the drain task exits on send failure).
        if bgra_tx.is_closed() {
            return;
        }
        // Subsequent frames: full 27-byte header.
        if stream.read_exact(&mut head_bytes[..1]).await.is_err() {
            break;
        }
        feedback.begin_frame();
        if !read_media_part(&mut stream, &mut head_bytes[1..]).await {
            break;
        }
        let Ok((hdr, _)) = parse_video_header(&head_bytes) else {
            tracing::warn!(head = ?head_bytes, "bad video header, dropping stream");
            break;
        };
        if !payload_len_ok(hdr.payload_len) {
            break;
        }
        let mut payload = vec![0u8; hdr.payload_len as usize];
        if !read_media_part(&mut stream, &mut payload).await {
            break;
        }

        feedback.received(hdr.pts_us, payload.len() + 27);

        // CONFIG_CHANGED or a resolution/codec change → hot-rebuild the decoder from
        // this keyframe.
        let stale = cur_cfg != Some((hdr.codec, hdr.width, hdr.height));
        let need_rebuild = decoder.is_none() || hdr.config_changed() || stale;
        if need_rebuild {
            if hdr.flags & video_flags::KEYFRAME == 0 {
                // Config changed but no parameter sets: request a keyframe and skip
                // this frame.
                request_keyframe(&cmd_tx, &last_kf);
                continue;
            }
            match rebuild_decoder(
                &hdr,
                &payload,
                bgra_tx.clone(),
                feedback.clone(),
                native_video,
            ) {
                Some(d) => {
                    tracing::info!(
                        codec = ?hdr.codec, w = hdr.width, h = hdr.height,
                        "video decoder (re)initialised"
                    );
                    // Tear down old callbacks before publishing the new decoder.
                    decoder = Some(d);
                    cur_cfg = Some((hdr.codec, hdr.width, hdr.height));
                }
                None => {
                    request_keyframe(&cmd_tx, &last_kf);
                    continue;
                }
            }
        }
        feedback.decoding(hdr.pts_us);
        let result = decoder
            .as_ref()
            .map(|d| d.decode_annexb(&payload, hdr.pts_us));
        let Some(result) = result else {
            feedback.decode_failed(hdr.pts_us);
            continue;
        };
        if let Err(e) = result {
            feedback.decode_failed(hdr.pts_us);
            // Mid-stream decode failure: nudge the host for a keyframe (rate-limited)
            // so recovery does not wait for the 2s periodic IDR (or forever on a
            // static screen).
            tracing::warn!(err=%e, "video decode failed, requesting keyframe");
            request_keyframe(&cmd_tx, &last_kf);
        }
    }
}

pub(super) async fn audio_recv_loop(
    mut stream: removent_net::quinn::RecvStream,
    pcm_tx: mpsc::Sender<crate::audio::DecodedAudio>,
) {
    // A blocked reliable read must not freeze the audio timeline. The playout
    // task conceals short gaps and expires retransmitted packets that are late.
    let jb = Arc::new(Mutex::new(JitterBuffer::new(60)));
    let arrived = Arc::new(tokio::sync::Notify::new());
    let _playout = AbortOnDrop(tokio::spawn(audio_playout_loop(
        jb.clone(),
        arrived.clone(),
        pcm_tx,
    )));

    // On the wire every packet is [14-byte header (incl. type byte)][payload] (same as
    // video frames); the first packet's type byte was already consumed by the media
    // dispatcher, so only the first packet needs it patched back in.
    let mut head = [0u8; 14];
    head[0] = removent_proto::STREAM_TYPE_AUDIO;
    let mut first = true;
    // Opt-in trace diagnostics are bounded per session and contain timing only.
    // Keep real capture cadence available for deterministic regression replay.
    let trace_started = Instant::now();
    let mut trace_remaining = 2_000u32;
    loop {
        let r = if first {
            first = false;
            read_media_part(&mut stream, &mut head[1..]).await
        } else {
            stream.read_exact(&mut head[..1]).await.is_ok()
                && read_media_part(&mut stream, &mut head[1..]).await
        };
        if !r {
            break;
        }
        if head[0] != removent_proto::STREAM_TYPE_AUDIO {
            tracing::warn!("audio stream type byte mismatch, dropping stream");
            break;
        }
        let Ok((hdr, _)) = parse_audio_header(&head) else {
            tracing::warn!("bad audio header, dropping stream");
            break;
        };
        let mut payload = vec![0u8; hdr.payload_len as usize];
        if !read_media_part(&mut stream, &mut payload).await {
            break;
        }
        let now = Instant::now();
        let mut buffer = jb.lock().unwrap();
        if trace_remaining > 0 {
            trace_remaining -= 1;
            let source_offset_us = buffer.source_time(hdr.pts_us).map(|source| {
                if now >= source {
                    now.duration_since(source).as_micros() as i64
                } else {
                    -(source.duration_since(now).as_micros() as i64)
                }
            });
            tracing::trace!(
                seq = hdr.seq,
                pts_us = hdr.pts_us,
                arrival_us = now.duration_since(trace_started).as_micros() as u64,
                source_offset_us,
                "audio arrival timing"
            );
        }
        buffer.push_at(
            AudioPacketIn {
                seq: hdr.seq,
                pts_us: hdr.pts_us,
                flags: hdr.flags,
                payload,
            },
            now,
        );
        drop(buffer);
        arrived.notify_one();
    }
}

/// Expire late media on source time; never block this clock behind a consumer.
/// The device renderer owns its own small prebuffer and clock-drift correction.
pub(super) async fn audio_playout_loop(
    jb: Arc<Mutex<JitterBuffer>>,
    arrived: Arc<tokio::sync::Notify>,
    pcm_tx: mpsc::Sender<crate::audio::DecodedAudio>,
) {
    let Ok(mut decoder) = AudioDecoder::new() else {
        tracing::warn!("audio decoder init failed");
        return;
    };
    let mut ticker = tokio::time::interval(Duration::from_millis(10));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut dropped_output = 0u64;
    let mut last_log = Instant::now();
    loop {
        tokio::select! {
            _ = pcm_tx.closed() => return,
            _ = ticker.tick() => {},
            // Decode arrivals immediately. The periodic tick only keeps loss
            // concealment alive when a reliable stream is stalled.
            _ = arrived.notified() => {},
        }
        // Decode a bounded capture batch together. Only the device callback
        // meters sample consumption; a second one-block timer would spread a
        // normal 40/60 ms capture batch and add another prebuffer of latency.
        for _ in 0..6 {
            let (outcome, timing) = {
                let mut buffer = jb.lock().unwrap();
                let outcome = buffer.pop();
                (outcome, buffer.output_timing())
            };
            let Some((pts_us, source_at, discontinuity)) = timing else {
                break;
            };
            if discontinuity {
                let Ok(fresh) = AudioDecoder::new() else {
                    return;
                };
                decoder = fresh;
            }
            let decoded = match outcome {
                PopOutcome::Packet(p) if p.flags & removent_proto::audio_flags::DTX == 0 => {
                    decoder.decode_frame(&p.payload)
                }
                PopOutcome::Packet(_) | PopOutcome::Plc(_) => decoder.conceal(),
                PopOutcome::Wait => break,
            };
            match decoded {
                Ok(samples) => {
                    let frame = crate::audio::DecodedAudio {
                        samples,
                        pts_us,
                        source_at,
                        discontinuity,
                    };
                    match pcm_tx.try_send(frame) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Closed(_)) => return,
                        Err(mpsc::error::TrySendError::Full(_)) => dropped_output += 1,
                    }
                }
                Err(error) => tracing::warn!(%error, "opus decode"),
            }
        }
        if last_log.elapsed() >= Duration::from_secs(5) {
            let buffer = jb.lock().unwrap();
            tracing::debug!(
                depth_ms = buffer.depth_ms(),
                late_dropped = buffer.late_dropped,
                plc_frames = buffer.concealed_frames,
                dropped_output,
                "audio delivery"
            );
            last_log = Instant::now();
        }
    }
}

use super::*;

#[tokio::test]
async fn adaptive_recovery_uses_real_static_refreshes_without_reducing_text_size() {
    let (_endpoints, client, host) = quic_pair().await;
    let (send, recv_client) = client.inner().open_bi().await.unwrap();
    let mut client_sink = ControlSink::new(send, removent_net::ControlCodec);
    client_sink
        .send(ControlMsg::Ping { ts_us: 0 })
        .await
        .unwrap();
    let (send, recv) = host.inner().accept_bi().await.unwrap();
    let mut source = ControlSource::new(recv, removent_net::ControlCodec);
    source.next().await.unwrap().unwrap();
    let mut controller = AdaptationController::with_dimensions(
        3000,
        30,
        removent_core::QualityPreset::Auto,
        320,
        240,
    );
    for _ in 0..20 {
        controller.on_sample(
            &Sample {
                rtt_ms: 0.,
                loss_pct: 10.,
                recv_kbps: 0,
                jitter_ms: 100.,
                decode_ms: 0.,
            },
            2500,
        );
    }
    assert_eq!(controller.state().scale, 1.0);
    let degraded = controller.state();
    let (quality_tx, quality_rx) = tokio::sync::watch::channel(controller.state());
    let health = Arc::new(crate::delivery::DeliveryHealth::new(host.clone()));
    let (cmd_tx, cmd_rx) = mpsc::channel(4);
    let cancel = CancellationToken::new();
    let _stop_on_drop = cancel.clone().drop_guard();
    let control = spawn_control_pump(
        source,
        ControlSink::new(send, removent_net::ControlCodec),
        ControlPumpDeps {
            conn: host.clone(),
            kf_tx: None,
            controller: Some(Arc::new(std::sync::Mutex::new(controller))),
            window_ms: 250,
            input: None,
            local_clip: None,
            quality_tx: Some(quality_tx),
            caps: Caps {
                input: false,
                ..Caps::all()
            },
            clip_state: None,
            cancel: cancel.clone(),
            peer_fp: None,
            delivery: Some(health.clone()),
        },
        cmd_rx,
    );
    let (frames, rx) = removent_core::latest::channel();
    let (_kf, kf_rx) = mpsc::channel(4);
    let video = spawn_video_loop(
        host.open_media_stream().await.unwrap(),
        rx,
        kf_rx,
        quality_rx,
        CodecId::H264,
        320,
        240,
        3000,
        30,
        cancel.clone(),
        None,
        0,
        Some(health),
        None,
    );
    frames
        .send(([40, 80, 160, 255].repeat(320 * 240), 0))
        .unwrap();
    let mut stream = client.accept_media_stream().await.unwrap();
    let (first, _) = video_packet(&mut stream).await;
    assert_eq!((first.width, first.height), (320, 240));
    let start = tokio::time::Instant::now();
    let recovered = tokio::time::timeout(Duration::from_secs(9), async {
        loop {
            let (header, _) = video_packet(&mut stream).await;
            if header.is_keyframe() {
                break header;
            }
        }
    })
    .await
    .unwrap();
    assert!(
        start.elapsed() >= Duration::from_secs(4),
        "must not immediately upgrade from one write"
    );
    assert_eq!((recovered.width, recovered.height), (320, 240));
    assert!(recovered.is_keyframe());
    let mut replies = ControlSource::new(recv_client, removent_net::ControlCodec);
    let reply = tokio::time::timeout(Duration::from_secs(1), replies.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(reply, ControlItem::Msg(msg) if matches!(*msg, ControlMsg::QualityControl { scale: 1.0, fps, bitrate_kbps } if fps >= degraded.fps && bitrate_kbps > degraded.bitrate_kbps))
    );
    drop(cmd_tx);
    cancel.cancel();
    video.await.unwrap();
    control.await.unwrap();
}

#[derive(Default)]
struct GeometryInput {
    dims: std::sync::Mutex<(u32, u32)>,
    points: std::sync::Mutex<Vec<(f32, f32)>>,
}
impl InputSink for GeometryInput {
    fn set_capture_dims(&self, w: u32, h: u32) {
        *self.dims.lock().unwrap() = (w, h);
    }
    fn mouse(&self, _: u64, x: f32, y: f32, _: u8, _: MouseKind) -> Result<(), String> {
        let (w, h) = *self.dims.lock().unwrap();
        self.points
            .lock()
            .unwrap()
            .push((x / w as f32, y / h as f32));
        Ok(())
    }
    fn key(
        &self,
        _: u16,
        _: removent_proto::KeyModifiers,
        _: KeyKind,
        _: Option<char>,
    ) -> Result<(), String> {
        Ok(())
    }
    fn scroll(&self, _: u64, _: f32, _: f32, _: ScrollPhase) -> Result<(), String> {
        Ok(())
    }
}

#[tokio::test]
async fn displayed_geometry_orders_with_mouse_input_across_resize() {
    let (_endpoints, client, host) = quic_pair().await;
    let (send, _recv) = client.inner().open_bi().await.unwrap();
    let mut sink = ControlSink::new(send, removent_net::ControlCodec);
    for (width, height) in [(320, 240), (160, 120), (320, 240)] {
        sink.send(ControlMsg::FrameGeometry { width, height })
            .await
            .unwrap();
        sink.send(ControlMsg::MouseEvent {
            display_id: 0,
            x_px: width as f32 / 2.,
            y_px: height as f32 / 2.,
            buttons: 0,
            kind: MouseKind::Moved,
        })
        .await
        .unwrap();
    }
    sink.send(ControlMsg::SessionEnd {
        reason: removent_proto::EndReason::ClientClosed,
    })
    .await
    .unwrap();
    let (send, recv) = host.inner().accept_bi().await.unwrap();
    let input = Arc::new(GeometryInput::default());
    let (_tx, rx) = mpsc::channel(4);
    let cancel = CancellationToken::new();
    let task = spawn_control_pump(
        ControlSource::new(recv, removent_net::ControlCodec),
        ControlSink::new(send, removent_net::ControlCodec),
        ControlPumpDeps {
            conn: host.clone(),
            kf_tx: None,
            controller: None,
            window_ms: 250,
            input: Some(input.clone()),
            local_clip: None,
            quality_tx: None,
            caps: Caps::all(),
            clip_state: None,
            cancel,
            peer_fp: None,
            delivery: None,
        },
        rx,
    );
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*input.points.lock().unwrap(), vec![(0.5, 0.5); 3]);
}

async fn video_packet(
    stream: &mut removent_net::quinn::RecvStream,
) -> (removent_proto::VideoFrameHeader, Vec<u8>) {
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut header = [0; 27];
        stream.read_exact(&mut header).await.unwrap();
        let (header, _) = removent_proto::parse_video_header(&header).unwrap();
        let mut payload = vec![0; header.payload_len as usize];
        stream.read_exact(&mut payload).await.unwrap();
        (header, payload)
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn static_recovery_probe_uses_temporal_compression_and_explicit_refresh_is_keyframe() {
    for codec in [CodecId::H264, CodecId::Hevc] {
        let (_endpoints, client, host) = quic_pair().await;
        let max = QualityState {
            bitrate_kbps: 3000,
            fps: 30,
            scale: 1.0,
        };
        let (tx, rx) = removent_core::latest::channel();
        let (kf_tx, kf_rx) = mpsc::channel(4);
        let (quality_tx, quality_rx) = tokio::sync::watch::channel(max);
        let cancel = CancellationToken::new();
        let _stop_on_drop = cancel.clone().drop_guard();
        let task = spawn_video_loop(
            host.open_media_stream().await.unwrap(),
            rx,
            kf_rx,
            quality_rx,
            codec,
            320,
            240,
            3000,
            30,
            cancel.clone(),
            None,
            0,
            None,
            None,
        );
        let frame: Vec<_> = (0..320 * 240)
            .flat_map(|i| {
                [
                    ((i % 320) / 8 * 7) as u8,
                    ((i / 320) / 8 * 5) as u8,
                    160,
                    255,
                ]
            })
            .collect();
        tx.send((frame, 10)).unwrap();
        let mut stream = client.accept_media_stream().await.unwrap();
        video_packet(&mut stream).await;
        quality_tx.send_replace(QualityState {
            bitrate_kbps: 1000,
            ..max
        });
        let (refresh, payload) = video_packet(&mut stream).await;
        assert!(refresh.is_keyframe());
        let params = removent_media_codec::extract_param_sets(&payload, codec == CodecId::Hevc);
        let decoder = removent_media_codec::VideoDecoder::new(codec, 320, 240, &params).unwrap();
        decoder.decode_annexb(&payload, refresh.pts_us).unwrap();
        let (probe, delta) = video_packet(&mut stream).await;
        assert!(
            !probe.is_keyframe(),
            "unchanged probe must preserve references"
        );
        assert!(
            delta.len() < payload.len(),
            "probe should avoid another full refresh"
        );
        decoder.decode_annexb(&delta, probe.pts_us).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(frame) = decoder.try_recv_decoded()
                    && frame.pts_us == probe.pts_us
                {
                    assert_eq!(frame.data.len(), 320 * 240 * 4);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        kf_tx.send(()).await.unwrap();
        let (requested, _) = video_packet(&mut stream).await;
        assert!(
            requested.is_keyframe(),
            "explicit decoder recovery still forces a keyframe"
        );
        cancel.cancel();
        task.await.unwrap();
    }
}

#[tokio::test]
async fn dynamic_quality_refreshes_static_frame_and_decodes_each_size() {
    for codec in [CodecId::H264, CodecId::Hevc, CodecId::Av1] {
        let (_endpoints, client, host) = quic_pair().await;
        let max = QualityState {
            bitrate_kbps: 3000,
            fps: 30,
            scale: 1.0,
        };
        let (tx, rx) = removent_core::latest::channel();
        let (_kf_tx, kf_rx) = mpsc::channel(4);
        let (quality_tx, quality_rx) = tokio::sync::watch::channel(max);
        let cancel = CancellationToken::new();
        let _stop_on_drop = cancel.clone().drop_guard();
        let task = spawn_video_loop(
            host.open_media_stream().await.unwrap(),
            rx,
            kf_rx,
            quality_rx,
            codec,
            320,
            240,
            3000,
            30,
            cancel.clone(),
            None,
            0,
            None,
            None,
        );
        tx.send(([40, 90, 160, 255].repeat(320 * 240), 10)).unwrap();
        let mut stream = client.accept_media_stream().await.unwrap();
        let (initial, _) = video_packet(&mut stream).await;
        assert_eq!((initial.width, initial.height), (320, 240));
        let mut previous_pts = initial.pts_us;
        for quality in [
            QualityState {
                bitrate_kbps: 1000,
                fps: 15,
                scale: 0.5,
            },
            QualityState {
                bitrate_kbps: 1000,
                fps: 15,
                scale: 0.75,
            },
            QualityState {
                bitrate_kbps: 1000,
                fps: 15,
                scale: 1.0,
            },
            max,
        ] {
            quality_tx.send_replace(quality);
            // No new captures: quality changes must refresh the cached screen.
            let (header, payload) = video_packet(&mut stream).await;
            let dims = scaled_dims((320, 240), quality.scale);
            assert_eq!((header.width as usize, header.height as usize), dims);
            assert!(header.is_keyframe() && header.config_changed());
            assert!(header.pts_us > previous_pts);
            previous_pts = header.pts_us;
            let params = if codec == CodecId::Av1 {
                Vec::new()
            } else {
                removent_media_codec::extract_param_sets(&payload, codec == CodecId::Hevc)
            };
            let decoder =
                removent_media_codec::VideoDecoder::new(codec, dims.0, dims.1, &params).unwrap();
            decoder.decode_annexb(&payload, header.pts_us).unwrap();
            let decoded = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if let Some(frame) = decoder.try_recv_decoded() {
                        break frame;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(decoded.data.len(), dims.0 * dims.1 * 4);
        }
        // A newer watch value replaces queued stale decisions, including recovery.
        quality_tx.send_replace(QualityState {
            fps: 15,
            scale: 0.5,
            ..max
        });
        quality_tx.send_replace(max);
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn dynamic_fps_limits_real_wire_frame_rate_under_capture_flood() {
    let (_endpoints, client, host) = quic_pair().await;
    let max = QualityState {
        bitrate_kbps: 3000,
        fps: 60,
        scale: 1.0,
    };
    let (tx, rx) = removent_core::latest::channel();
    let (_kf_tx, kf_rx) = mpsc::channel(4);
    let (quality_tx, quality_rx) = tokio::sync::watch::channel(max);
    let cancel = CancellationToken::new();
    let _stop_on_drop = cancel.clone().drop_guard();
    let task = spawn_video_loop(
        host.open_media_stream().await.unwrap(),
        rx,
        kf_rx,
        quality_rx,
        CodecId::H264,
        64,
        64,
        3000,
        60,
        cancel.clone(),
        None,
        0,
        None,
        None,
    );
    tx.send(([0, 80, 170, 255].repeat(64 * 64), 0)).unwrap();
    let mut stream = client.accept_media_stream().await.unwrap();
    video_packet(&mut stream).await;
    quality_tx.send_replace(QualityState { fps: 15, ..max });
    video_packet(&mut stream).await; // configuration keyframe
    let stop = cancel.clone();
    let producer = tokio::spawn(async move {
        let mut pts = 1;
        loop {
            tokio::select! {
                _ = stop.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_millis(1)) => {
                    if tx.send(([pts as u8, 80, 170, 255].repeat(64 * 64), pts)).is_err() { break; }
                    pts += 1;
                }
            }
        }
    });
    let start = tokio::time::Instant::now();
    // Observe ten complete frames; at 15 fps they cannot arrive as a burst.
    for _ in 0..10 {
        video_packet(&mut stream).await;
    }
    assert!(start.elapsed() >= Duration::from_millis(550));
    cancel.cancel();
    task.await.unwrap();
    producer.await.unwrap();
}

#[tokio::test]
async fn raw_backlog_coalesces_to_latest_without_dropping_future_capture() {
    let (tx, mut rx) = mpsc::channel(4);
    for frame in 1..=4 {
        tx.send(frame).await.unwrap();
    }
    let first = rx.recv().await.unwrap();
    assert_eq!(newest_queued(first, &mut rx), 4);
    assert!(rx.is_empty());
    tx.send(5).await.unwrap();
    assert_eq!(rx.recv().await, Some(5));
}

async fn quic_pair() -> (
    [removent_net::quinn::Endpoint; 2],
    RvpConnection,
    RvpConnection,
) {
    let dir = tempfile::tempdir().unwrap();
    let identity = removent_core::identity::load_or_create(
        &removent_core::DataPaths {
            root: dir.path().to_owned(),
        },
        "stall-test",
    )
    .unwrap();
    let (client, _) = removent_net::make_client_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &identity,
        removent_net::PinState::new([], true),
    )
    .unwrap();
    let (host, _) = removent_net::make_server_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &identity,
        removent_net::PinState::new([], true),
    )
    .unwrap();
    let (c, h) = tokio::join!(
        client
            .connect(host.local_addr().unwrap(), "removent")
            .unwrap(),
        async { host.accept().await.unwrap().await }
    );
    (
        [client, host],
        RvpConnection::new(c.unwrap()),
        RvpConnection::new(h.unwrap()),
    )
}

#[tokio::test]
async fn blocked_control_write_releases_inputs_on_cancel_abort_and_timeout() {
    use crate::input_sink::{RecordedInput, RecorderInputSink};
    use removent_proto::{KeyKind, KeyModifiers};
    for mode in 0..3 {
        let (_endpoints, client, host) = quic_pair().await;
        let (send, _recv) = client.inner().open_bi().await.unwrap();
        let mut client_sink = ControlSink::new(send, removent_net::ControlCodec);
        client_sink
            .send(ControlMsg::KeyEvent {
                vk_code: 0,
                modifiers: KeyModifiers::empty(),
                kind: KeyKind::Down,
                unicode: None,
            })
            .await
            .unwrap();
        let (send, recv) = host.inner().accept_bi().await.unwrap();
        // Exhaust the sender's write budget while QUIC itself remains live.
        host.inner().set_send_window(0);
        let recorder = Arc::new(RecorderInputSink::default());
        let cancel = CancellationToken::new();
        let (tx, rx) = mpsc::channel(4);
        let mut task = spawn_control_pump(
            ControlSource::new(recv, removent_net::ControlCodec),
            ControlSink::new(send, removent_net::ControlCodec),
            ControlPumpDeps {
                conn: host.clone(),
                kf_tx: None,
                controller: None,
                window_ms: 250,
                input: Some(recorder.clone()),
                local_clip: None,
                quality_tx: None,
                caps: Caps::all(),
                clip_state: None,
                cancel: cancel.clone(),
                peer_fp: None,
                delivery: None,
            },
            rx,
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            while recorder.events.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tx.send(ControlMsg::Ping { ts_us: 1 }).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while tx.capacity() != 4 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!task.is_finished());
        if mode == 0 {
            // A blocked outgoing Pong/command must not prevent a real
            // incoming release from being injected before teardown.
            client_sink
                .send(ControlMsg::KeyEvent {
                    vk_code: 0,
                    modifiers: KeyModifiers::empty(),
                    kind: KeyKind::Up,
                    unicode: None,
                })
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_millis(500), async {
                while recorder.events.lock().unwrap().len() != 2 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("outgoing congestion blocked an incoming key release");
            assert!(!task.is_finished());
        }
        match mode {
            0 => cancel.cancel(),
            1 => task.abort(),
            _ => {} // Keep the connection live; the write deadline must fire.
        }
        let result = tokio::time::timeout(Duration::from_secs(12), &mut task)
            .await
            .unwrap();
        assert_eq!(result.is_err(), mode == 1);
        assert!(cancel.is_cancelled());
        let events = recorder.events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[1],
            RecordedInput::Key {
                kind: KeyKind::Up,
                ..
            }
        ));
    }
}

#[tokio::test]
async fn blocked_audio_write_exits_on_cancel() {
    let (_endpoints, _client, host) = quic_pair().await;
    host.inner().set_send_window(0);
    let stream = host.open_media_stream().await.unwrap();
    let (tx, rx) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    let mut task = spawn_audio_loop(stream, rx, 96, cancel.clone());
    tx.send(removent_media_capture::AudioFrame {
        samples: vec![0; 960],
        pts_micros: 1,
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while tx.capacity() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!task.is_finished());
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(1), &mut task)
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn token_bucket_allows_burst_up_to_capacity() {
    let t0 = std::time::Instant::now();
    let mut b = TokenBucket::new(100.0);
    b.last = t0;
    for _ in 0..100 {
        assert!(b.try_take_at(t0), "burst within capacity should pass");
    }
    assert!(!b.try_take_at(t0), "capacity exhausted should drop");
}

#[test]
fn token_bucket_refills_over_time() {
    let t0 = std::time::Instant::now();
    let mut b = TokenBucket::new(600.0);
    b.last = t0;
    for _ in 0..600 {
        assert!(b.try_take_at(t0));
    }
    assert!(!b.try_take_at(t0));
    // 100ms later: 60 tokens refilled.
    let t1 = t0 + Duration::from_millis(100);
    for _ in 0..60 {
        assert!(b.try_take_at(t1));
    }
    assert!(!b.try_take_at(t1));
}

#[test]
fn token_bucket_caps_refill_at_capacity() {
    let t0 = std::time::Instant::now();
    let mut b = TokenBucket::new(10.0);
    b.last = t0;
    // Long idle: tokens saturate at capacity, no unbounded accumulation.
    let t1 = t0 + Duration::from_secs(60);
    for _ in 0..10 {
        assert!(b.try_take_at(t1));
    }
    assert!(!b.try_take_at(t1));
}

#[test]
fn fit_capture_dims_preserves_aspect_ratio() {
    // 16:10 Retina panel: height is the binding constraint → 1728×1080,
    // not the old stretched 1920×1080.
    assert_eq!(fit_capture_dims(2880, 1800), (1728, 1080));
    // 16:9 exactly at the box: unchanged.
    assert_eq!(fit_capture_dims(1920, 1080), (1920, 1080));
    // 4K 16:9: width binds.
    assert_eq!(fit_capture_dims(3840, 2160), (1920, 1080));
    // Smaller than the box: never upscale.
    assert_eq!(fit_capture_dims(1280, 720), (1280, 720));
}

#[test]
fn fit_capture_dims_forces_even_and_nonzero() {
    // 1440×900 @1x fits as-is (already even); an odd fit must round down.
    let (w, h) = fit_capture_dims(2881, 1801);
    assert_eq!(w % 2, 0);
    assert_eq!(h % 2, 0);
    assert!(w >= 2 && h >= 2);
}

#[test]
fn delayed_packet_commits_its_own_submitted_pixels() {
    let frame_a: Arc<[u8]> = Arc::from([1, 2, 3, 4]);
    let frame_b: Arc<[u8]> = Arc::from([5, 6, 7, 8]);
    let mut submitted = HashMap::new();
    submitted.insert(10, frame_a.clone());
    submitted.insert(20, frame_b.clone());
    let mut dedup = FrameDeduplicator::new();

    // AV1 can return A while the encode call is currently submitting B.
    mark_submitted_frame_sent(&mut dedup, &mut submitted, 10);
    assert!(!dedup.should_encode(&frame_a, false));
    assert!(dedup.should_encode(&frame_b, false));

    mark_submitted_frame_sent(&mut dedup, &mut submitted, 20);
    assert!(!dedup.should_encode(&frame_b, false));
}

#[test]
fn pairing_begin_limiter_blocks_rapid_retries() {
    let t0 = std::time::Instant::now();
    let mut l = PairingBeginLimiter {
        last_begin: HashMap::new(),
    };
    assert!(l.allow("peer-a", t0));
    // Immediate reconnect from the same peer: blocked (no PIN popup).
    assert!(!l.allow("peer-a", t0 + Duration::from_secs(1)));
    // A different peer is unaffected.
    assert!(l.allow("peer-b", t0 + Duration::from_secs(1)));
    // After the interval the peer may pair again.
    assert!(l.allow("peer-a", t0 + PAIRING_BEGIN_MIN_INTERVAL));
}

fn test_ack() -> NegotiateAck {
    NegotiateAck {
        video: VideoParams {
            codec: CodecId::H264,
            max_fps: 30,
            max_bitrate_kbps: 1_000,
            initial_scale: 1.0,
        },
        audio: removent_proto::AudioParams::default(),
        resume_token: None,
    }
}

/// §7.3: the 30s window runs from session end ("peer last seen"), so a session
/// longer than the window must still be resumable once it ends.
#[test]
fn refresh_resume_reanchors_window_to_session_end() {
    let fp = "fp-refresh-test";
    let token = [3u8; 16];
    remember_resume(fp, &token, &test_ack(), Caps::all(), None);
    // Simulate a long session: issuance is backdated to the edge of the window.
    {
        let mut store = resume_store().lock().unwrap();
        store.get_mut(fp).unwrap().issued_at = now_unix() - (RESUME_WINDOW_SECS - 1);
    }
    // The session ends now: the window re-anchors to this moment.
    refresh_resume(fp);
    {
        let store = resume_store().lock().unwrap();
        let issued_at = store.get(fp).unwrap().issued_at;
        assert!(
            now_unix().saturating_sub(issued_at) <= 1,
            "issued_at must be re-anchored to session end"
        );
    }
    assert!(validate_resume(fp, &token).is_some());
    invalidate_resume(fp);
}

/// §7.4: a lost rotation reply must not strand the client — the previous
/// generation validates once, then is retired for good.
#[test]
fn previous_generation_token_is_tolerated_once() {
    let fp = "fp-prev-gen-test";
    let t1 = [1u8; 16];
    let t2 = [2u8; 16];
    let t3 = [4u8; 16];
    // Rotation after consuming t1: t2 is current, t1 is the tolerated previous generation.
    remember_resume(fp, &t2, &test_ack(), Caps::all(), Some(t1));
    let (_, _, matched_prev) = validate_resume_full(fp, &t1).expect("prev generation validates");
    assert!(matched_prev);
    let (_, _, matched_prev) = validate_resume_full(fp, &t2).expect("current token validates");
    assert!(!matched_prev);
    // Consuming the previous generation rotates without a prev: t1 is retired.
    invalidate_resume(fp);
    remember_resume(fp, &t3, &test_ack(), Caps::all(), None);
    assert!(validate_resume_full(fp, &t1).is_none());
    assert!(validate_resume_full(fp, &t2).is_none());
    invalidate_resume(fp);
}

#[test]
fn resume_policy_requires_same_authentication_credentials() {
    let fp = "authentication-policy-resume-test";
    let token = [73; 16];
    let ack = removent_proto::NegotiateAck {
        video: removent_proto::VideoParams {
            codec: CodecId::H264,
            max_bitrate_kbps: 3000,
            max_fps: 30,
            initial_scale: 1.0,
        },
        audio: removent_proto::AudioParams {
            enabled: false,
            sample_rate: 48000,
            channels: 2,
            bitrate_kbps: 64,
            frame_ms: 10,
        },
        resume_token: Some(token),
    };
    let mut auth = removent_core::AuthenticationSettings {
        mode: removent_core::AuthenticationMode::Password,
        password: "first".into(),
        ..Default::default()
    };
    remember_resume(fp, &token, &ack, Caps::all(), None);
    remember_resume_policy(fp, &auth);
    assert!(resume_policy_matches(fp, &auth));
    auth.password = "second".into();
    assert!(!resume_policy_matches(fp, &auth));
    auth.mode = removent_core::AuthenticationMode::None;
    assert!(!resume_policy_matches(fp, &auth));
    invalidate_resume(fp);
}

#[test]
fn every_connection_policy_rejects_even_matching_fresh_resume_token() {
    let fp = "every-connection-resume-test";
    let token = [91; 16];
    let auth = removent_core::AuthenticationSettings {
        pairing_policy: removent_core::authentication::PairingPolicy::EveryConnection,
        ..Default::default()
    };
    remember_resume(fp, &token, &test_ack(), Caps::all(), None);
    remember_resume_policy(fp, &auth);
    assert!(validate_resume_full(fp, &token).is_some());
    assert!(!resume_policy_matches(fp, &auth));
    invalidate_resume(fp);
}

#[tokio::test]
async fn resume_handshake_rechecks_revoked_trust_and_reduced_capability_grants() {
    for trusted in [true, false] {
        let (_endpoints, client, host) = quic_pair().await;
        let fp = hex::encode(host.peer_fingerprint().unwrap());
        let token = [47; 16];
        let auth = removent_core::AuthenticationSettings::default();
        remember_resume(&fp, &token, &test_ack(), Caps::all(), None);
        remember_resume_policy(&fp, &auth);
        let mut peers = PeersStore::in_memory();
        peers
            .upsert(PeerRecord {
                fingerprint: fp.clone(),
                short_fp: fp[..16].into(),
                name: "Restricted".into(),
                trusted,
                granted_caps: Caps {
                    video: true,
                    ..Caps::none()
                },
                added_at_unix: 0,
                last_connected_unix: 0,
            })
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let id = removent_core::identity::load_or_create(
            &removent_core::DataPaths {
                root: dir.path().into(),
            },
            "ResumePolicyTest",
        )
        .unwrap();
        let task = tokio::spawn(async move {
            serve_connection(
                host,
                &id,
                &mut peers,
                &HostConfig {
                    authentication: auth,
                    auth_paths: None,
                    audio_available: false,
                    preapproved_only: false,
                    device_name: "ResumePolicyTest".into(),
                    admission: AdmissionMode::TrustedAuto,
                    video_bitrate_kbps: 1000,
                    video_fps: 30,
                    input_sink: Some(Arc::new(crate::input_sink::RecorderInputSink::default())),
                    local_clip: None,
                },
                HostInteractions {
                    show_pairing_pin: Box::new(|_| {}),
                    admission_prompt: Box::new(|_, _| Box::pin(async { false })),
                },
                removent_proto::DisplayInfo {
                    id: 1,
                    w_px: 320,
                    h_px: 240,
                    scale: 1.,
                    dpi: 96,
                    is_main: true,
                },
            )
            .await
        });
        let (ack, _sink, _source) = client
            .connect_handshake(removent_proto::HandshakeClient {
                magic: removent_proto::MAGIC,
                proto_version: PROTO_VERSION,
                feature_bits: removent_proto::feature_bits::AUTH_METHODS,
                hello: removent_proto::Hello {
                    app_version: "test".into(),
                    device_name: "Restricted".into(),
                    os_version: "test".into(),
                    caps: Caps::all(),
                    resume_token: Some(token),
                },
            })
            .await
            .unwrap();
        assert_eq!(
            ack.resume_accepted,
            Some(false),
            "A fresh token must not bypass updated grants or trust"
        );
        client.inner().close(0u32.into(), b"test complete");
        task.abort();
        let _ = task.await;
        invalidate_resume(&fp);
    }
}

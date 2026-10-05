use super::*;
use removent_net::{ControlCodec, PinState, make_client_endpoint, make_server_endpoint};
use removent_proto::{AudioParams, CodecId, VideoParams};

struct Peer {
    _endpoints: [removent_net::quinn::Endpoint; 2],
    conn: RvpConnection,
    sink: ControlSink,
    source: ControlSource,
}

async fn fixture(
    caps: Caps,
    clip: Option<Arc<dyn removent_core::TextClipboard>>,
) -> (ClientSession, Peer) {
    let client_dir = tempfile::tempdir().unwrap();
    let host_dir = tempfile::tempdir().unwrap();
    let identity = |dir: &tempfile::TempDir| {
        removent_core::identity::load_or_create(
            &removent_core::DataPaths {
                root: dir.path().to_owned(),
            },
            "lifecycle-test",
        )
        .unwrap()
    };
    let (client, _) = make_client_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &identity(&client_dir),
        PinState::new([], true),
    )
    .unwrap();
    let (host, _) = make_server_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &identity(&host_dir),
        PinState::new([], true),
    )
    .unwrap();
    let (client_conn, host_conn) = tokio::join!(
        client
            .connect(host.local_addr().unwrap(), "removent")
            .unwrap(),
        async { host.accept().await.unwrap().await },
    );
    let client_conn = RvpConnection::new(client_conn.unwrap());
    let host_conn = RvpConnection::new(host_conn.unwrap());
    let (send, recv) = client_conn.inner().open_bi().await.unwrap();
    let mut sink = ControlSink::new(send, ControlCodec);
    sink.send(ControlMsg::Ping { ts_us: 0 }).await.unwrap();
    let (send, recv_host) = host_conn.inner().accept_bi().await.unwrap();
    let mut source = ControlSource::new(recv_host, ControlCodec);
    source.next().await.unwrap().unwrap();
    let ack = NegotiateAck {
        video: VideoParams {
            codec: CodecId::Hevc,
            max_fps: 30,
            max_bitrate_kbps: 3000,
            initial_scale: 1.,
        },
        audio: AudioParams {
            enabled: caps.audio,
            ..Default::default()
        },
        resume_token: None,
    };
    let session = build_session(
        client_conn,
        sink,
        ControlSource::new(recv, ControlCodec),
        ack,
        ClientConfig {
            pairing_code: None,
            device_name: "test".into(),
            caps,
            local_clip: clip,
        },
    )
    .unwrap();
    (
        session,
        Peer {
            _endpoints: [client, host],
            conn: host_conn,
            sink: ControlSink::new(send, ControlCodec),
            source,
        },
    )
}

#[tokio::test]
async fn receiver_stall_drives_host_adaptation_and_keeps_input_live() {
    use removent_core::{AdaptationController, QualityPreset};
    use removent_media_codec::VideoEncoder;
    use removent_proto::{CodecId, KeyKind, VideoFrameHeader, build_video_frame, video_flags};
    let caps = Caps {
        audio: false,
        ..Caps::all()
    };
    let (mut session, peer) = fixture(caps, None).await;
    let controller = Arc::new(Mutex::new(AdaptationController::with_dimensions(
        3000,
        30,
        QualityPreset::Auto,
        320,
        240,
    )));
    let (quality_tx, quality_rx) = tokio::sync::watch::channel(controller.lock().unwrap().state());
    let input = Arc::new(removent_host::RecorderInputSink::default());
    let (_commands, commands_rx) = mpsc::channel(4);
    let (keyframes, keyframes_rx) = mpsc::channel(4);
    let deps = removent_host::ControlPumpDeps {
        conn: peer.conn.clone(),
        kf_tx: None,
        controller: Some(controller.clone()),
        window_ms: 250,
        input: Some(input.clone()),
        local_clip: None,
        quality_tx: Some(quality_tx),
        caps,
        clip_state: None,
        cancel: Default::default(),
        peer_fp: None,
        // Deliberately omit sender evidence: only real receiver reports
        // can cause adaptation in this test.
        delivery: None,
    };
    let cancel = deps.cancel.clone();
    let _pump = AbortOnDrop(removent_host::spawn_control_pump(
        peer.source,
        peer.sink,
        deps,
        commands_rx,
    ));
    let mut stream = peer.conn.open_media_stream().await.unwrap();
    stream
        .write_all(&[removent_proto::STREAM_TYPE_VIDEO])
        .await
        .unwrap();
    // Leave a partial frame pending, as ordered-stream retransmission can.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if session.last_quality.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("automatic receiver feedback must reach the host");
    let degraded = controller.lock().unwrap().state();
    assert!(degraded.bitrate_kbps < 3000 && degraded.fps < 30);
    assert_eq!(degraded.scale, 1.);
    // Cold encoders on virtualized Macs can leave the partial frame pending
    // through another downgrade. Exercise that case before starting video.
    tokio::time::timeout(Duration::from_secs(5), async {
        while controller.lock().unwrap().state().bitrate_kbps >= degraded.bitrate_kbps {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("a sustained receiver stall must continue lowering the bitrate");
    for kind in [KeyKind::Down, KeyKind::Up] {
        session
            .send(ControlMsg::KeyEvent {
                vk_code: 0,
                modifiers: Default::default(),
                kind,
                unicode: None,
            })
            .await
            .unwrap();
    }
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if input.events.lock().unwrap().len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("key release must arrive while video is still incomplete");
    assert!(matches!(
        input.events.lock().unwrap()[1],
        removent_host::RecordedInput::Key {
            kind: KeyKind::Up,
            ..
        }
    ));
    let mut encoder = VideoEncoder::new(CodecId::Hevc, 320, 240, 3000, 30).unwrap();
    let raw = [40, 80, 160, 255].repeat(320 * 240);
    let frame = encoder.encode_bgra(&raw, 0).unwrap().remove(0);
    let packet = build_video_frame(
        &VideoFrameHeader {
            frame_id: 0,
            pts_us: 0,
            flags: video_flags::KEYFRAME,
            codec: CodecId::Hevc,
            width: 320,
            height: 240,
            payload_len: frame.data.len() as u32,
        },
        &frame.data,
    );
    stream.write_all(&packet[1..]).await.unwrap();
    let decoded = tokio::time::timeout(Duration::from_secs(3), session.decoded_bgra_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!((decoded.width, decoded.height), (320, 240));
    let (frames, frames_rx) = removent_core::latest::channel();
    let _video = AbortOnDrop(removent_host::spawn_video_loop(
        stream,
        frames_rx,
        keyframes_rx,
        quality_rx,
        CodecId::Hevc,
        320,
        240,
        3000,
        30,
        cancel.clone(),
        None,
        0,
        None,
        None,
    ));
    // Decode real cached-screen refreshes. Wall-clock capture/arrival jitter
    // on a shared CI runner is not a deterministic healthy-network fixture.
    // Keyframe requests keep receiver evidence fresh without inventing stats.
    frames.send((raw, 1)).unwrap();
    let _refreshes = AbortOnDrop(tokio::spawn(async move {
        let _keep_capture_open = frames;
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if keyframes.send(()).await.is_err() {
                break;
            }
        }
    }));
    // Recovery is an increase from the actual floor, not necessarily above
    // the first downgrade: startup may have caused several more reductions,
    // and each healthy upgrade deliberately takes five seconds.
    let mut lowest_bitrate = controller.lock().unwrap().state().bitrate_kbps;
    let recovery = tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let frame = session
                .decoded_bgra_rx
                .recv()
                .await
                .expect("video remains live");
            assert_eq!((frame.width, frame.height), (320, 240));
            let bitrate = controller.lock().unwrap().state().bitrate_kbps;
            lowest_bitrate = lowest_bitrate.min(bitrate);
            if bitrate > lowest_bitrate {
                break;
            }
        }
    })
    .await;
    assert!(
        recovery.is_ok(),
        "healthy decoded traffic must permit recovery: first={degraded:?}, \
         lowest_bitrate={lowest_bitrate}, current={:?}",
        controller.lock().unwrap().state()
    );
    cancel.cancel();
}

#[tokio::test]
async fn missing_media_streams_close_viewer_despite_live_control() {
    let (mut session, _peer) = fixture(Caps::all(), None).await;
    assert!(
        tokio::time::timeout(Duration::from_secs(12), session.decoded_bgra_rx.recv())
            .await
            .unwrap()
            .is_none()
    );
    assert!(session.was_interrupted());
}

#[tokio::test]
async fn irrelevant_messages_do_not_extend_negotiation_deadline() {
    let (session, mut peer) = fixture(Caps::all(), None).await;
    let tx = session.cmd_tx.clone();
    let _producer = AbortOnDrop(tokio::spawn(async move {
        loop {
            if tx.send(ControlMsg::Ping { ts_us: 1 }).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }));
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        expect_msg(
            &mut peer.source,
            "negotiation",
            Duration::from_millis(100),
            |msg| matches!(msg, ControlMsg::SessionAccept),
        ),
    )
    .await
    .expect("unrelated traffic must not reset the deadline");
    assert!(matches!(result, Err(ConnectError::Timeout("negotiation"))));
}

#[tokio::test]
async fn peer_end_or_control_eof_closes_pending_media_receivers() {
    for eof in [false, true] {
        let (mut session, mut peer) = fixture(Caps::all(), None).await;
        // Start a video stream but leave its frame header incomplete and
        // never open audio: both the dispatcher and a child are waiting.
        let mut video = peer.conn.open_media_stream().await.unwrap();
        video
            .write_all(&[removent_proto::STREAM_TYPE_VIDEO])
            .await
            .unwrap();
        if eof {
            peer.sink.get_mut().finish().unwrap();
        } else {
            peer.sink
                .send(ControlMsg::SessionEnd {
                    reason: EndReason::HostClosed,
                })
                .await
                .unwrap();
        }
        assert!(
            tokio::time::timeout(Duration::from_secs(2), session.decoded_bgra_rx.recv())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(2), session.decoded_pcm_rx.recv())
                .await
                .unwrap()
                .is_none()
        );
        assert!(session.conn.inner().close_reason().is_some());
        assert_eq!(
            session.was_interrupted(),
            eof,
            "EOF must remain retryable after local cleanup"
        );
    }
}

#[tokio::test]
async fn close_flushes_session_end_before_aborting_pump() {
    let (session, mut peer) = fixture(Caps::all(), None).await;
    session.close().await.unwrap();
    let end = tokio::time::timeout(Duration::from_secs(2), peer.source.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(end, ControlItem::Msg(msg) if matches!(*msg, ControlMsg::SessionEnd { reason: EndReason::ClientClosed }))
    );
}

#[tokio::test]
async fn disabled_or_non_text_clipboard_is_acknowledged_without_applying() {
    use removent_core::TextClipboard;
    for enabled in [false, true] {
        let clip = removent_core::MemoryClipboard::new();
        clip.write("local").unwrap();
        let caps = Caps {
            clipboard: enabled,
            ..Caps::all()
        };
        let (_session, mut peer) = fixture(caps, Some(clip.clone())).await;
        for (seq, format) in [
            (1, removent_proto::ClipFormat::Html),
            (2, removent_proto::ClipFormat::TextUtf8),
        ] {
            peer.sink
                .send(ControlMsg::ClipboardSync {
                    seq,
                    format,
                    data: b"remote".to_vec(),
                })
                .await
                .unwrap();
            let ack = tokio::time::timeout(Duration::from_secs(2), peer.source.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(
                matches!(ack, ControlItem::Msg(msg) if matches!(*msg, ControlMsg::ClipboardAck { seq: received } if received == seq))
            );
            assert_eq!(
                clip.read().unwrap(),
                if enabled && seq == 2 {
                    "remote"
                } else {
                    "local"
                }
            );
        }
        if !enabled {
            clip.write("new local copy").unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(350), peer.source.next())
                    .await
                    .is_err()
            );
        }
    }
}

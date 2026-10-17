use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
async fn display_is_prepared_after_admission_including_resume() {
    for (denied, missing, resume, quality) in [
        (true, false, false, None),
        (false, true, false, None),
        (false, false, false, None),
        (false, false, true, None),
        (
            false,
            false,
            false,
            Some(removent_proto::VideoQuality::Ultra),
        ),
        (
            false,
            false,
            true,
            Some(removent_proto::VideoQuality::Ultra),
        ),
        (false, false, false, Some(removent_proto::VideoQuality::Low)),
    ] {
        let (_endpoints, client, host) = crate::session::tests::quic_pair().await;
        let dir = tempfile::tempdir().unwrap();
        let identity = removent_core::identity::load_or_create(
            &removent_core::DataPaths {
                root: dir.path().into(),
            },
            "display-test",
        )
        .unwrap();
        let mut peers = PeersStore::in_memory();
        let cfg = HostConfig {
            authentication: removent_core::AuthenticationSettings {
                mode: removent_core::AuthenticationMode::None,
                ..Default::default()
            },
            auth_paths: None,
            audio_available: false,
            preapproved_only: false,
            device_name: "display-test".into(),
            admission: if denied {
                AdmissionMode::DenyAll
            } else {
                AdmissionMode::TrustedAuto
            },
            video_bitrate_kbps: 1000,
            video_fps: 30,
            input_sink: None,
            local_clip: None,
        };
        let fp = hex::encode(host.peer_fingerprint().unwrap());
        let token = [62; 16];
        if resume {
            let ack = NegotiateAck {
                video: VideoParams {
                    codec: CodecId::Hevc,
                    max_fps: 240,
                    max_bitrate_kbps: 256_000,
                    initial_scale: 1.,
                },
                audio: removent_proto::AudioParams {
                    enabled: false,
                    ..Default::default()
                },
                resume_token: Some(token),
            };
            remember_resume(&fp, &token, &ack, Caps::all(), None);
            remember_resume_policy(&fp, &cfg.authentication);
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let prepared = calls.clone();
        let host_conn = host.clone();
        let task = tokio::spawn(async move {
            serve_connection_with_display(
                host_conn,
                &identity,
                &mut peers,
                &cfg,
                HostInteractions {
                    show_pairing_pin: Box::new(|_| panic!("no pairing expected")),
                    admission_prompt: Box::new(|_, _| Box::pin(async { false })),
                },
                || async move {
                    prepared.fetch_add(1, Ordering::SeqCst);
                    if missing {
                        return Err(HostError::Timeout("active capture display after wake"));
                    }
                    Ok(removent_proto::DisplayInfo {
                        id: 2,
                        w_px: 1920,
                        h_px: 1080,
                        scale: 1.,
                        dpi: 96,
                        is_main: true,
                    })
                },
            )
            .await
        });
        let (hello, mut sink, mut source) = client
            .connect_handshake(removent_proto::HandshakeClient {
                magic: removent_proto::MAGIC,
                proto_version: PROTO_VERSION,
                feature_bits: removent_proto::feature_bits::AUTH_METHODS
                    | if quality.is_some() {
                        removent_proto::feature_bits::DYNAMIC_VIDEO
                    } else {
                        0
                    },
                hello: removent_proto::Hello {
                    app_version: "test".into(),
                    device_name: "test".into(),
                    os_version: "test".into(),
                    caps: Caps::all(),
                    resume_token: resume.then_some(token),
                },
            })
            .await
            .unwrap();
        if let Some(quality) = quality {
            assert_ne!(
                hello.feature_bits & removent_proto::feature_bits::DYNAMIC_VIDEO,
                0
            );
            sink.send(ControlMsg::VideoPreferences { quality })
                .await
                .unwrap();
        }
        if resume {
            assert_eq!(hello.resume_accepted, Some(true));
        }
        if !denied && !resume {
            sink.send(ControlMsg::SessionRequest { caps: Caps::all() })
                .await
                .unwrap();
        }
        let reply = expect_msg(&mut source, "admission", |_| true)
            .await
            .unwrap();
        if denied {
            assert!(matches!(reply, ControlMsg::SessionReject { .. }));
        } else if missing {
            assert!(matches!(
                reply,
                ControlMsg::Error {
                    code: removent_proto::ErrorCode::DisplayGone
                }
            ));
        } else {
            assert!(matches!(reply, ControlMsg::SessionAccept));
            if !resume {
                let ControlMsg::NegotiateOffer { n } =
                    expect_msg(&mut source, "offer", |_| true).await.unwrap()
                else {
                    panic!("expected offer")
                };
                assert_eq!(n.selected_display, 2);
                assert_eq!(n.displays[0].id, 2);
                sink.send(ControlMsg::NegotiateReply {
                    ack: Box::new(NegotiateAck {
                        video: n.video,
                        audio: n.audio,
                        resume_token: None,
                    }),
                })
                .await
                .unwrap();
            }
            assert!(matches!(
                expect_msg(&mut source, "final ack", |_| true)
                    .await
                    .unwrap(),
                ControlMsg::NegotiateReply { .. }
            ));
        }
        let result = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.is_ok(), !denied && !missing);
        if let Ok((established, ..)) = result {
            assert_eq!(established.video_quality, quality);
            assert!(established.ack.video.max_fps <= 30);
            assert!(established.ack.video.max_bitrate_kbps <= 1000);
        }
        assert_eq!(calls.load(Ordering::SeqCst), usize::from(!denied));
        invalidate_resume(&fp);
    }
}

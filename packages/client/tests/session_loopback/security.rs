use super::*;
use futures::{SinkExt, StreamExt};
use removent_proto::{ControlMsg, HandshakeServer, Negotiate, VideoParams};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_accept_cannot_skip_confirmation_or_invitation_proof() {
    for invitation in [false, true] {
        let (client_id, _cd) = identity_for("UnverifiedClient");
        let (host_id, _hd) = identity_for("UnverifiedHost");
        let (server, _) = make_server_endpoint(
            "127.0.0.1:0".parse().unwrap(),
            &host_id,
            PinState::new([], true),
        )
        .unwrap();
        let addr = server.local_addr().unwrap();
        let host = tokio::spawn(async move {
            let conn = RvpConnection::new(server.accept().await.unwrap().await.unwrap());
            let (_, mut control, mut requests) = conn
                .accept_handshake(|_| HandshakeServer {
                    proto_version: removent_proto::PROTO_VERSION,
                    feature_bits: removent_proto::feature_bits::AUTH_METHODS,
                    device_name: "UnverifiedHost".into(),
                    peer_known: invitation, // An invitation must ignore this claim.
                    resume_accepted: invitation.then_some(true),
                })
                .await
                .unwrap();
            let (mut pair, mut source) = conn.accept_pairing().await.unwrap();
            let begin = source.next().await.unwrap().unwrap();
            let hs = removent_net::host_on_begin_with_secret(&begin, "654321").unwrap();
            pair.send(hs.reply).await.unwrap();
            source.next().await.unwrap().unwrap(); // Client proof; deliberately never confirmed.
            requests.next().await.unwrap().unwrap(); // SessionRequest
            control.send(ControlMsg::SessionAccept).await.unwrap();
            control
                .send(ControlMsg::NegotiateOffer {
                    n: Box::new(Negotiate {
                        displays: vec![],
                        selected_display: 1,
                        video: VideoParams {
                            codec: CodecId::Hevc,
                            max_fps: 30,
                            max_bitrate_kbps: 3000,
                            initial_scale: 1.0,
                        },
                        audio: removent_proto::AudioParams {
                            enabled: false,
                            ..Default::default()
                        },
                    }),
                })
                .await
                .unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(1200), requests.next())
                    .await
                    .is_err(),
                "Client must not negotiate before receiving a verified confirmation"
            );
            pair.get_mut().finish().unwrap();
            conn.inner().closed().await;
        });
        let (ep, _) = make_client_endpoint(
            "127.0.0.1:0".parse().unwrap(),
            &client_id,
            PinState::new([], true),
        )
        .unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            connect_session(
                ep,
                addr,
                &client_id,
                ClientConfig {
                    pairing_code: invitation.then(|| {
                        removent_core::pairing_invitation::PairingCode::parse("123456654321")
                            .unwrap()
                    }),
                    device_name: "UnverifiedClient".into(),
                    caps: Caps::all(),
                    local_clip: None,
                },
                invitation.then_some([9; 16]),
                None,
                Some(Box::new(|_, tx| {
                    let _ = tx.send("654321".into());
                })),
            ),
        )
        .await
        .unwrap();
        assert!(
            matches!(result, Err(removent_client::ConnectError::Pairing(_))),
            "Unverified host must be rejected"
        );
        tokio::time::timeout(Duration::from_secs(2), host)
            .await
            .unwrap()
            .unwrap();
    }
}

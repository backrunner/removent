//! Loopback integration tests: QUIC mutual-TLS connection → RVP handshake → control stream → pairing stream → media stream.

use futures::{SinkExt, StreamExt};
use removent_core::{DataPaths, DeviceIdentity, identity};
use removent_net::{
    ControlItem, PairingMsg, PinState, RvpConnection, client_begin, client_confirm_check,
    client_verify, host_on_begin, host_verify, make_client_endpoint, make_server_endpoint,
};
use removent_proto::{
    Caps, CodecId, ControlMsg, HandshakeClient, HandshakeServer, Hello, MAGIC, PROTO_VERSION,
    VideoFrameHeader, build_video_frame, parse_video_header, video_flags,
};
use std::time::Duration;
use tokio::time::timeout;

fn identity_for(name: &str) -> (DeviceIdentity, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let p = DataPaths {
        root: dir.path().to_path_buf(),
    };
    (identity::load_or_create(&p, name).unwrap(), dir)
}

/// Establish a mutual-TLS QUIC connection pair (both sides allow_unknown, simulating a first connection).
type Endpoints = (quinn::Endpoint, quinn::Endpoint);

async fn setup_pair() -> (
    RvpConnection,
    RvpConnection,
    DeviceIdentity,
    DeviceIdentity,
    Endpoints,
    tempfile::TempDir,
    tempfile::TempDir,
) {
    let (client_id, cd) = identity_for("ClientLoop");
    let (host_id, hd) = identity_for("HostLoop");

    let (ep_server, _server_pin) = make_server_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &host_id,
        PinState::new([], true),
    )
    .unwrap();
    let _unused = &_server_pin;
    let server_addr = ep_server.local_addr().unwrap();

    // Critical: accept must be driven concurrently with connect, otherwise the QUIC handshake cannot complete.
    let ep_server_for_accept = ep_server.clone();
    let acceptor = tokio::spawn(async move {
        let inc = timeout(Duration::from_secs(5), ep_server_for_accept.accept())
            .await
            .expect("accept timeout")
            .expect("no incoming");
        timeout(Duration::from_secs(5), inc)
            .await
            .expect("incoming handshake timeout")
            .expect("incoming handshake")
    });

    let (ep_client, _client_pin) = make_client_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &client_id,
        PinState::new([], true),
    )
    .unwrap();

    let conn_c = timeout(
        Duration::from_secs(5),
        ep_client.connect(server_addr, "removent").unwrap(),
    )
    .await
    .expect("connect timeout")
    .expect("quinn connect");

    let conn_h = acceptor.await.expect("acceptor task panicked");

    (
        RvpConnection::new(conn_c),
        RvpConnection::new(conn_h),
        client_id,
        host_id,
        (ep_client, ep_server),
        cd,
        hd,
    )
}

fn sample_hello(device_name: &str) -> HandshakeClient {
    HandshakeClient {
        magic: MAGIC,
        proto_version: PROTO_VERSION,
        feature_bits: 0b11,
        hello: Hello {
            app_version: "0.1.0".into(),
            device_name: device_name.to_string(),
            os_version: "macOS".into(),
            caps: Caps::all(),
            resume_token: None,
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn minimal_stream_echo() {
    let (client, host, _cid, _hid, _eps, _cd, _hd) = setup_pair().await;
    let s = client.inner().clone();
    let h = host.inner().clone();

    let server = tokio::spawn(async move {
        let (mut send, mut recv) = h.accept_bi().await.unwrap();
        let mut buf = vec![0u8; 5];
        recv.read_exact(&mut buf).await.unwrap();
        send.write_all(b"WORLD").await.unwrap();
    });

    let (mut c_send, mut c_recv) = s.open_bi().await.unwrap();
    c_send.write_all(b"HELLO").await.unwrap();
    let mut buf = vec![0u8; 5];
    match tokio::time::timeout(Duration::from_secs(3), c_recv.read_exact(&mut buf)).await {
        Ok(Ok(())) => eprintln!("[m-c] got {buf:?}"),
        other => panic!("[m-c] recv failed: {other:?}"),
    }
    server.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bare_connect_succeeds() {
    let (client_id, _cd) = identity_for("BareClient");
    let (host_id, _hd) = identity_for("BareHost");
    let (ep_server, _server_pin) = make_server_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &host_id,
        PinState::new([], true),
    )
    .unwrap();
    let _unused = &_server_pin;
    let addr = ep_server.local_addr().unwrap();
    eprintln!("server at {addr}");

    let acceptor = tokio::spawn(async move {
        match ep_server.accept().await {
            Some(inc) => match inc.await {
                Ok(_) => eprintln!("SERVER CONNECTED"),
                Err(e) => eprintln!("SERVER HANDSHAKE FAIL: {e}"),
            },
            None => eprintln!("SERVER CLOSED"),
        }
    });

    let (ep_client, _client_pin) = make_client_endpoint(
        "127.0.0.1:0".parse().unwrap(),
        &client_id,
        PinState::new([], true),
    )
    .unwrap();
    match ep_client.connect(addr, "removent").unwrap().await {
        Ok(_) => eprintln!("CLIENT CONNECTED"),
        Err(e) => panic!("CLIENT FAIL: {e}"),
    }
    acceptor.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handshake_control_and_media_roundtrip() {
    let (client, host, _cid, _hid, _eps, _cd, _hd) = setup_pair().await;

    let _host_alive = host.clone();
    let host_task = tokio::spawn(async move {
        let ack = HandshakeServer {
            proto_version: PROTO_VERSION,
            feature_bits: 0b11,
            device_name: "HostLoop".into(),
            resume_accepted: None,
            peer_known: false,
        };
        let (hello, mut sink, mut source) =
            timeout(Duration::from_secs(5), host.accept_handshake(|_| ack))
                .await
                .expect("[H] accept_handshake timeout")
                .unwrap();
        // Keep consuming client control messages (this is the control loop in the real engine).
        tokio::spawn(async move { while let Some(Ok(_)) = source.next().await {} });
        assert_eq!(hello.hello.device_name, "ClientMac");

        sink.send(ControlMsg::Pong { ts_us: 12345 }).await.unwrap();
        let _ = sink.get_mut().finish();

        let mut ms = host.open_media_stream().await.unwrap();
        let hdr = VideoFrameHeader {
            frame_id: 7,
            pts_us: 42_000,
            flags: video_flags::KEYFRAME | video_flags::CONFIG_CHANGED,
            codec: CodecId::Hevc,
            width: 1920,
            height: 1080,
            payload_len: 8,
        };
        ms.write_all(&build_video_frame(&hdr, b"NALUDATA"))
            .await
            .unwrap();
        let _ = ms.finish();
    });

    // ---- Client: handshake → Ping → receive video frame ----
    let handshake_fut = client.connect_handshake(sample_hello("ClientMac"));
    let (ack, mut sink, mut source) = timeout(Duration::from_secs(5), handshake_fut)
        .await
        .expect("[C] handshake timeout")
        .unwrap();
    assert_eq!(ack.proto_version, PROTO_VERSION);
    assert_eq!(ack.device_name, "HostLoop");
    assert!(ack.resume_accepted.is_none());

    sink.send(ControlMsg::Ping { ts_us: 12345 }).await.unwrap();
    let item = timeout(Duration::from_secs(3), source.next())
        .await
        .expect("pong timeout")
        .expect("stream ok")
        .expect("decode ok");
    match item {
        ControlItem::Msg(m) if matches!(*m, ControlMsg::Pong { ts_us: 12345 }) => {}
        other => panic!("expected Pong(12345), got {other:?}"),
    }

    let mut media = client.accept_media_stream().await.unwrap();
    let mut head_buf = vec![0u8; 27];
    timeout(Duration::from_secs(3), media.read_exact(&mut head_buf))
        .await
        .expect("video header timeout")
        .expect("read header");
    let (hdr, consumed) = parse_video_header(&head_buf).unwrap();
    assert_eq!(consumed, 27);
    assert_eq!(hdr.frame_id, 7);
    assert!(hdr.is_keyframe() && hdr.config_changed());
    assert_eq!(hdr.width, 1920);

    let mut payload = vec![0u8; hdr.payload_len as usize];
    timeout(Duration::from_secs(3), media.read_exact(&mut payload))
        .await
        .expect("payload timeout")
        .expect("read payload");
    assert_eq!(&payload, b"NALUDATA");

    host_task.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_handshake_times_out_when_host_never_replies() {
    let (client, host, _cid, _hid, _eps, _cd, _hd) = setup_pair().await;
    // A busy host never accepts the control stream, so no HandshakeServer reply ever
    // arrives; the client must fail with a timeout instead of hanging forever.
    let _host_alive = host;
    let started = std::time::Instant::now();
    let err = client
        .connect_handshake(sample_hello("BusyClient"))
        .await
        .expect_err("handshake must fail when the host never replies");
    assert!(
        matches!(err, removent_net::NetError::Timeout),
        "expected NetError::Timeout, got {err:?}"
    );
    assert!(
        started.elapsed() >= Duration::from_secs(12),
        "timed out too early: {:?}",
        started.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pairing_over_quinn_stream_succeeds_with_real_pin() {
    let (client, host, _cid, _hid, _eps, _cd, _hd) = setup_pair().await;
    let (client_id, _c2) = identity_for("PairClient");
    let (host_id, _h2) = identity_for("PairHost");
    let fp_c = client_id.fingerprint_hex();
    let fp_h = host_id.fingerprint_hex();

    // The PIN is passed from the server task to the client via a oneshot channel (in the real scenario the user copies it by eye).
    let (pin_tx, pin_rx) = tokio::sync::oneshot::channel::<String>();

    let _host_alive = host.clone();
    let host_host_id = host_id.clone();
    let host_fp_c = fp_c.clone();

    let host_task = tokio::spawn(async move {
        let (mut sink, mut source) = host.accept_pairing().await.unwrap();
        let begin = source.next().await.unwrap().unwrap();
        let hs = host_on_begin(&begin).unwrap();
        pin_tx.send(hs.pin.clone()).unwrap();
        sink.send(hs.reply.clone()).await.unwrap();

        let verify = source.next().await.unwrap().unwrap();
        let (confirm, shared) = host_verify(hs, &verify, &host_host_id, &host_fp_c).unwrap();
        assert!(
            shared.is_some(),
            "host pairing must succeed with matching pin"
        );
        sink.send(confirm).await.unwrap();
        let _ = sink.get_mut().finish();
    });

    let (mut psink, mut psource) = client.open_pairing().await.unwrap();
    let (begin, nonce_c) = client_begin(&fp_c);
    psink.send(begin).await.unwrap();

    let challenge = timeout(Duration::from_secs(3), psource.next())
        .await
        .expect("challenge timeout")
        .expect("stream ok")
        .expect("decode ok");
    let PairingMsg::Challenge { nonce_h, .. } = &challenge else {
        panic!("expected Challenge")
    };
    let nonce_h = *nonce_h;

    let pin = timeout(Duration::from_secs(3), pin_rx)
        .await
        .unwrap()
        .unwrap();
    let (verify, shared_c) =
        client_verify(&nonce_c, &challenge, &fp_c, &fp_h, &pin, &client_id).unwrap();
    psink.send(verify).await.unwrap();

    let confirm = timeout(Duration::from_secs(3), psource.next())
        .await
        .expect("confirm timeout")
        .expect("stream ok")
        .expect("decode ok");
    client_confirm_check(
        &shared_c,
        &nonce_c,
        &nonce_h,
        &fp_c,
        &fp_h,
        &confirm,
        &host_id.verifying_key(),
    )
    .expect("pairing confirm should succeed");

    timeout(Duration::from_secs(5), host_task)
        .await
        .unwrap()
        .unwrap();
}

use super::*;
use aes::Aes128;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, KeyInit};
use md5::{Digest, Md5};
use num_bigint::BigUint;
use removent_proto::{ControlMsg, KeyModifiers, MouseKind, ScrollPhase};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::{Duration, timeout};

#[test]
fn des_password_vector_matches_vnc_spec() {
    let challenge = *b"0123456789abcdef";
    assert_eq!(
        hex::encode(vnc_response(b"password", &challenge)),
        "5645abeb5f1e6475e8feb11beb66ea19"
    );
}

#[test]
fn mac_virtual_keys_map_to_x11_keysyms() {
    assert_eq!(
        keysym_for_key(0x00, KeyModifiers::empty(), None),
        'a' as u32
    );
    assert_eq!(keysym_for_key(0x00, KeyModifiers::SHIFT, None), 'A' as u32);
    assert_eq!(keysym_for_key(0x7b, KeyModifiers::empty(), None), 0xff51);
    assert_eq!(keysym_for_key(0x38, KeyModifiers::empty(), None), 0xffe1);
}

#[test]
fn mouse_button_order_matches_rfb() {
    // Internal order is left/right/middle; RFB always swaps the latter two.
    assert_eq!(rfb_button_mask(0b001), 0b001);
    assert_eq!(rfb_button_mask(0b010), 0b100);
    assert_eq!(rfb_button_mask(0b100), 0b010);
    assert_eq!(rfb_button_mask(0b111), 0b111);
}

#[tokio::test]
async fn pointer_packets_keep_rfb_button_order_with_standard_and_apple_banners() {
    // RFC 6143 section 7.5.5: PointerEvent always uses left/middle/right
    // bits. Apple's greeting must not switch these ordinary type-5 packets
    // to the macOS event API's left/right/middle order.
    for banner in [RFB_VERSION, ARD_VERSION] {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            stream.write_all(banner).await.unwrap();
            let mut version = [0; 12];
            stream.read_exact(&mut version).await.unwrap();
            assert_eq!(&version, RFB_VERSION);
            stream.write_all(&[1, SEC_NONE]).await.unwrap();
            assert_eq!(stream.read_u8().await.unwrap(), SEC_NONE);
            stream.write_all(&0u32.to_be_bytes()).await.unwrap();
            assert_eq!(stream.read_u8().await.unwrap(), 1);
            let mut init = [0; 24];
            init[..2].copy_from_slice(&320u16.to_be_bytes());
            init[2..4].copy_from_slice(&200u16.to_be_bytes());
            init[4..20].copy_from_slice(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
            stream.write_all(&init).await.unwrap();
            let _ = read_client_setup(&mut stream).await;

            // Keep the framebuffer stalled while observing the actual input
            // socket: right-down/drag, scroll with right held, right-up,
            // middle-click, then a left/right chord with separate releases.
            for (mask, x, y) in [
                (4, 10, 20),
                (4, 11, 21),
                (12, 11, 21),
                (4, 11, 21),
                (68, 11, 21),
                (4, 11, 21),
                (0, 11, 21),
                (2, 11, 21),
                (0, 11, 21),
                (1, 11, 21),
                (5, 11, 21),
                (4, 11, 21),
                (0, 11, 21),
            ] {
                let mut packet = [0; 6];
                timeout(Duration::from_secs(2), stream.read_exact(&mut packet))
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(packet, [5, mask, 0, x, 0, y], "banner: {banner:?}");
            }
        });
        let session = connect_vnc(addr, "").await.unwrap();
        for (index, (buttons, kind, x, y)) in [
            (2, MouseKind::RightDown, 10., 20.),
            (2, MouseKind::Moved, 11., 21.),
            (0, MouseKind::RightUp, 11., 21.),
            (4, MouseKind::MiddleDown, 11., 21.),
            (0, MouseKind::MiddleUp, 11., 21.),
            (1, MouseKind::LeftDown, 11., 21.),
            (3, MouseKind::RightDown, 11., 21.),
            (2, MouseKind::LeftUp, 11., 21.),
            (0, MouseKind::RightUp, 11., 21.),
        ]
        .into_iter()
        .enumerate()
        {
            session
                .cmd_tx
                .try_send(ControlMsg::MouseEvent {
                    display_id: 0,
                    x_px: x,
                    y_px: y,
                    buttons,
                    kind,
                })
                .unwrap();
            if index == 1 {
                session
                    .cmd_tx
                    .try_send(ControlMsg::ScrollEvent {
                        display_id: 0,
                        dx_mm: 3.,
                        dy_mm: -3.,
                        phase: ScrollPhase::Changed,
                    })
                    .unwrap();
            }
        }
        timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap();
    }
}

#[test]
fn color_components_scale_to_eight_bits() {
    assert_eq!(scale_component(0, 31), 0);
    assert_eq!(scale_component(31, 31), 255);
    assert_eq!(scale_component(15, 31), 123);
}

#[test]
fn ard_credentials_are_nul_terminated_and_zero_padded() {
    let mut credentials = [0u8; 128];
    copy_c_string(&mut credentials[..64], b"alice", "username").unwrap();
    copy_c_string(&mut credentials[64..], b"secret", "password").unwrap();
    assert_eq!(&credentials[..6], b"alice\0");
    assert!(credentials[6..64].iter().all(|&b| b == 0));
    assert_eq!(&credentials[64..71], b"secret\0");
    assert!(credentials[71..].iter().all(|&b| b == 0));
}

#[test]
fn ard_credentials_reject_overlong_fields() {
    let mut field = [0u8; 64];
    assert!(copy_c_string(&mut field, &[b'x'; 64], "username").is_err());
    assert!(copy_c_string(&mut field, &[b'x'; 63], "username").is_ok());
}

#[test]
fn apple_security_prefers_dh_when_account_credentials_are_present() {
    // Real macOS offer: selecting type 35 first stalls its handshake.
    assert_eq!(
        choose_security_type(&[30, 33, 36, 31, 32, 2, 35], "alice", b"pw").unwrap(),
        SEC_ARD
    );
    assert_eq!(
        choose_security_type(&[35, 2, 30], "alice", b"pw").unwrap(),
        SEC_ARD
    );

    assert_eq!(
        choose_security_type(&[SEC_NONE, SEC_ARD], "alice", b"pw").unwrap(),
        SEC_ARD
    );
    // Type 35 is not the type-30 DH protocol. Never select an unimplemented
    // method and then stall waiting for a challenge that will not arrive.
    assert!(choose_security_type(&[SEC_ARD_MACOS], "alice", b"pw").is_err());
    assert_eq!(
        choose_security_type(&[SEC_ARD_MACOS, SEC_VNC_AUTH], "alice", b"pw").unwrap(),
        SEC_VNC_AUTH
    );
    assert_eq!(
        choose_security_type(&[SEC_VNC_AUTH, SEC_ARD], "", b"pw").unwrap(),
        SEC_VNC_AUTH
    );
    assert!(choose_security_type(&[SEC_ARD], "", b"").is_err());
}

#[test]
fn rfb_version_negotiation_falls_back_to_supported_wire_versions() {
    assert_eq!(
        negotiated_version(b"RFB 003.003\n"),
        Some(b"RFB 003.003\n".as_slice())
    );
    assert_eq!(
        negotiated_version(b"RFB 003.006\n"),
        Some(b"RFB 003.003\n".as_slice())
    );
    assert_eq!(
        negotiated_version(b"RFB 003.007\n"),
        Some(b"RFB 003.007\n".as_slice())
    );
    assert_eq!(
        negotiated_version(b"RFB 003.889\n"),
        Some(RFB_VERSION.as_slice())
    );
    assert_eq!(
        negotiated_version(b"RFB 004.000\n"),
        Some(RFB_VERSION.as_slice())
    );
    assert!(negotiated_version(b"not a vnc!!!").is_none());
    assert!(negotiated_version(b"RFB 003x008\n").is_none());
    assert!(negotiated_version(b"RFB 003.008x").is_none());
}

#[tokio::test]
async fn standard_versions_and_security_methods_interoperate() {
    for version in [b"RFB 003.003\n", b"RFB 003.007\n", b"RFB 003.008\n"] {
        for security in [SEC_NONE, SEC_VNC_AUTH] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                stream.write_all(version).await.unwrap();
                let mut reply = [0; 12];
                stream.read_exact(&mut reply).await.unwrap();
                assert_eq!(&reply, version);
                if version == b"RFB 003.003\n" {
                    stream.write_u32(u32::from(security)).await.unwrap();
                } else {
                    stream.write_all(&[1, security]).await.unwrap();
                    assert_eq!(stream.read_u8().await.unwrap(), security);
                }
                if security == SEC_VNC_AUTH {
                    let challenge = *b"0123456789abcdef";
                    stream.write_all(&challenge).await.unwrap();
                    let mut response = [0; 16];
                    stream.read_exact(&mut response).await.unwrap();
                    assert_eq!(response, vnc_response(b"password", &challenge));
                }
                if security != SEC_NONE || version == RFB_VERSION {
                    stream.write_u32(0).await.unwrap();
                }
                assert_eq!(stream.read_u8().await.unwrap(), 1);
                let mut init = [0; 24];
                init[1] = 1;
                init[3] = 1;
                stream.write_all(&init).await.unwrap();
                read_client_setup(&mut stream).await;
            });
            let session = timeout(Duration::from_secs(2), connect_vnc(addr, "password")).await;
            assert!(
                matches!(session, Ok(Ok(_))),
                "{version:?}, security {security}"
            );
            server.await.unwrap();
        }
    }
}

#[tokio::test]
async fn legacy_security_type_cannot_truncate_to_none() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream.write_all(b"RFB 003.003\n").await.unwrap();
        let mut reply = [0; 12];
        stream.read_exact(&mut reply).await.unwrap();
        stream.write_u32(257).await.unwrap();
        // Unknown u32 type 257 must not become None (1) and emit ClientInit.
        assert_eq!(stream.read(&mut [0]).await.unwrap(), 0);
    });
    assert!(matches!(
        timeout(Duration::from_secs(2), connect_vnc(addr, ""))
            .await
            .unwrap(),
        Err(VncError::Protocol(_))
    ));
    server.await.unwrap();
}

#[test]
fn ard_dh_values_use_fixed_width_big_endian_encoding() {
    let value = BigUint::from(0x1234u32);
    assert_eq!(fixed_be_bytes(&value, 4).unwrap(), vec![0, 0, 0x12, 0x34]);
    assert!(fixed_be_bytes(&value, 1).is_err());
}

#[tokio::test]
async fn client_handshake_and_raw_frame_roundtrip() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream.write_all(RFB_VERSION).await.unwrap();
        let mut client_version = [0u8; 12];
        stream.read_exact(&mut client_version).await.unwrap();
        stream.write_all(&[1, SEC_NONE]).await.unwrap();
        let mut selected = [0u8; 1];
        stream.read_exact(&mut selected).await.unwrap();
        assert_eq!(selected[0], SEC_NONE);
        stream.write_all(&0u32.to_be_bytes()).await.unwrap();
        let mut shared = [0u8; 1];
        stream.read_exact(&mut shared).await.unwrap();
        let mut init = vec![0u8; 24];
        init[1] = 1;
        init[3] = 1;
        init[4..20].copy_from_slice(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
        stream.write_all(&init).await.unwrap();
        let _ = read_client_setup(&mut stream).await;
        let mut update = vec![0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0];
        update.extend_from_slice(&[1, 2, 3, 255]);
        stream.write_all(&update).await.unwrap();
    });
    use crate::connection::{ConnectionProgress, ConnectionStage};
    let stages = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let collected = stages.clone();
    let progress: ConnectionProgress = std::sync::Arc::new(move |stage| {
        collected.lock().unwrap().push(stage);
    });
    let mut session = connect_vnc_with_progress(addr, "", "", Some(&progress))
        .await
        .unwrap();
    assert_eq!(
        *stages.lock().unwrap(),
        vec![
            ConnectionStage::Connecting,
            ConnectionStage::Negotiating,
            ConnectionStage::Authenticating,
            ConnectionStage::PreparingDesktop,
        ]
    );
    let frame = timeout(Duration::from_secs(1), session.decoded_bgra_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(frame.width, 1);
    assert_eq!(frame.height, 1);
    assert_eq!(frame.data, vec![1, 2, 3, 255]);
    server.await.unwrap();
}

#[tokio::test]
async fn apple_ard_type30_handshake_pointer_and_raw_frame_roundtrip() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream.write_all(ARD_VERSION).await.unwrap();
        let mut client_version = [0u8; 12];
        stream.read_exact(&mut client_version).await.unwrap();
        assert_eq!(&client_version, RFB_VERSION);
        stream
            .write_all(&[7, 30, 33, 36, 31, 32, 2, 35])
            .await
            .unwrap();
        let mut selected = [0u8; 1];
        stream.read_exact(&mut selected).await.unwrap();
        assert_eq!(selected[0], SEC_ARD);

        // Deterministic 128-bit DH group: p=2^128-159, g=5,
        // server private=6. Apple production groups are larger, but the
        // fixture keeps the handshake fast while exercising fixed-width
        // encoding and the minimum accepted key size.
        let prime = (BigUint::from(1u8) << 128) - 159u8;
        let server_public = BigUint::from(5u8).modpow(&BigUint::from(6u8), &prime);
        stream.write_all(&[0, 5, 0, 16]).await.unwrap();
        stream
            .write_all(&fixed_be_bytes(&prime, 16).unwrap())
            .await
            .unwrap();
        stream
            .write_all(&fixed_be_bytes(&server_public, 16).unwrap())
            .await
            .unwrap();
        let mut encrypted_credentials = [0u8; 128];
        let mut client_public = [0u8; 16];
        stream.read_exact(&mut encrypted_credentials).await.unwrap();
        stream.read_exact(&mut client_public).await.unwrap();
        let shared = BigUint::from_bytes_be(&client_public).modpow(&BigUint::from(6u8), &prime);
        let mut digest = Md5::new();
        digest.update(fixed_be_bytes(&shared, 16).unwrap());
        let key = digest.finalize();
        let cipher = Aes128::new_from_slice(&key).unwrap();
        let mut decrypted = encrypted_credentials;
        for block in decrypted.as_chunks_mut::<16>().0 {
            cipher.decrypt_block(GenericArray::from_mut_slice(block));
        }
        assert_eq!(&decrypted[..6], b"alice\0");
        assert_eq!(&decrypted[64..71], b"secret\0");
        stream.write_all(&0u32.to_be_bytes()).await.unwrap();

        let mut client_init = [0u8; 1];
        stream.read_exact(&mut client_init).await.unwrap();
        assert_eq!(client_init[0], 1);
        let mut init = vec![0u8; 24];
        init[1] = 1;
        init[3] = 1;
        init[4..20].copy_from_slice(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
        init[23] = 3;
        stream.write_all(&init).await.unwrap();
        stream.write_all(b"ARD").await.unwrap();

        let request = read_client_setup(&mut stream).await;
        assert_eq!(request[0], 3); // FramebufferUpdateRequest

        let mut click = [0; 12];
        timeout(Duration::from_secs(2), stream.read_exact(&mut click))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(click, [5, 4, 0, 0, 0, 0, 5, 0, 0, 0, 0, 0]);

        let mut update = vec![0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0];
        update.extend_from_slice(&[1, 2, 3, 255]);
        stream.write_all(&update).await.unwrap();
    });
    let mut session = connect_vnc_with_credentials(addr, "alice", "secret")
        .await
        .unwrap();
    for (buttons, kind) in [(2, MouseKind::RightDown), (0, MouseKind::RightUp)] {
        session
            .cmd_tx
            .try_send(ControlMsg::MouseEvent {
                display_id: 0,
                x_px: 0.,
                y_px: 0.,
                buttons,
                kind,
            })
            .unwrap();
    }
    let frame = timeout(Duration::from_secs(1), session.decoded_bgra_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(frame.width, 1);
    assert_eq!(frame.height, 1);
    assert_eq!(frame.data, vec![1, 2, 3, 255]);
    server.await.unwrap();
}

#[tokio::test]
async fn apple_ard_session_select_and_zero_size_frame_roundtrip() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream.write_all(ARD_VERSION).await.unwrap();
        let mut client_version = [0u8; 12];
        stream.read_exact(&mut client_version).await.unwrap();
        stream.write_all(&[1, SEC_NONE]).await.unwrap();
        let mut selected = [0u8; 1];
        stream.read_exact(&mut selected).await.unwrap();
        stream.write_all(&0u32.to_be_bytes()).await.unwrap();
        let mut client_init = [0u8; 1];
        stream.read_exact(&mut client_init).await.unwrap();
        assert_eq!(client_init[0], 1);

        // Extended ServerInit: zero dimensions and session-select flag.
        let mut init = vec![0u8; 24];
        init[4..20].copy_from_slice(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
        let mut name = vec![0u8, 0, 0, 0, 0, 4];
        name.extend_from_slice(&[0u8; 16]);
        name.extend_from_slice(&[0, b'A', b'R', b'D']);
        init[20..24].copy_from_slice(&(name.len() as u32).to_be_bytes());
        stream.write_all(&init).await.unwrap();
        stream.write_all(&name).await.unwrap();

        // SessionInfo: version, allowed commands (request/console/virtual),
        // reserved, and the currently logged-in console user.
        let user = b"alice\0";
        let body_size = 10 + user.len();
        stream
            .write_all(&(body_size as u16).to_be_bytes())
            .await
            .unwrap();
        stream.write_all(&1u16.to_be_bytes()).await.unwrap();
        stream.write_all(&7u32.to_be_bytes()).await.unwrap();
        stream.write_all(&0u32.to_be_bytes()).await.unwrap();
        stream.write_all(user).await.unwrap();

        let mut command = [0u8; 74];
        stream.read_exact(&mut command).await.unwrap();
        assert_eq!(&command[0..2], &72u16.to_be_bytes());
        assert_eq!(command[8], ARD_SESSION_CMD_CONNECT_CONSOLE);
        assert_eq!(&command[10..16], user);

        // Granted SessionResult (version 1, status 0).
        stream.write_all(&6u16.to_be_bytes()).await.unwrap();
        stream.write_all(&1u16.to_be_bytes()).await.unwrap();
        stream
            .write_all(&ARD_SESSION_STATUS_GRANTED.to_be_bytes())
            .await
            .unwrap();

        // Standard SetPixelFormat, SetEncodings, then the safe max-size
        // probe request.
        let request = read_client_setup(&mut stream).await;
        assert_eq!(&request[6..8], &u16::MAX.to_be_bytes());
        assert_eq!(&request[8..10], &u16::MAX.to_be_bytes());

        let mut update = vec![
            0, 0, 0, 1, // FramebufferUpdate + one rectangle
            0, 0, 0, 0, // x/y
            0, 1, 0, 1, // width/height
            0, 0, 0, 0, // Raw encoding
        ];
        update.extend_from_slice(&[1, 2, 3, 255]);
        stream.write_all(&update).await.unwrap();
    });
    let mut session = connect_vnc_with_credentials(addr, "", "").await.unwrap();
    let frame = timeout(Duration::from_secs(1), session.decoded_bgra_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(frame.width, 1);
    assert_eq!(frame.height, 1);
    assert_eq!(frame.data, vec![1, 2, 3, 255]);
    server.await.unwrap();
}

#[tokio::test]
async fn closing_command_channel_closes_frames_while_server_stays_open() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let (keep_open_tx, keep_open_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream.write_all(RFB_VERSION).await.unwrap();
        let mut client_version = [0u8; 12];
        stream.read_exact(&mut client_version).await.unwrap();
        stream.write_all(&[1, SEC_NONE]).await.unwrap();
        let mut selected = [0u8; 1];
        stream.read_exact(&mut selected).await.unwrap();
        assert_eq!(selected[0], SEC_NONE);
        stream.write_all(&0u32.to_be_bytes()).await.unwrap();
        let mut shared = [0u8; 1];
        stream.read_exact(&mut shared).await.unwrap();
        let mut init = vec![0u8; 24];
        init[1] = 1;
        init[3] = 1;
        init[4..20].copy_from_slice(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
        stream.write_all(&init).await.unwrap();
        let _ = read_client_setup(&mut stream).await;
        let mut update = vec![0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0];
        update.extend_from_slice(&[1, 2, 3, 255]);
        stream.write_all(&update).await.unwrap();
        // The server leaves its write half open until the test finishes.
        let _ = keep_open_rx.await;
    });
    let mut session = connect_vnc(addr, "").await.unwrap();
    let frame = timeout(Duration::from_secs(1), session.decoded_bgra_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(frame.width, 1);
    assert_eq!(frame.height, 1);
    assert_eq!(frame.data, vec![1, 2, 3, 255]);
    let (replacement, _unused) = queue::channel();
    drop(std::mem::replace(&mut session.cmd_tx, replacement));
    assert!(
        timeout(Duration::from_secs(1), session.decoded_bgra_rx.recv())
            .await
            .expect("writer shutdown must close the frame channel")
            .is_none()
    );
    let _ = keep_open_tx.send(());
    server.await.unwrap();
}

#[tokio::test]
async fn key_burst_reaches_server_before_a_stalled_4k_update_finishes() {
    use removent_proto::{ControlMsg, KeyKind};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (partial_tx, partial_rx) = tokio::sync::oneshot::channel();
    let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream.write_all(RFB_VERSION).await.unwrap();
        let mut version = [0; 12];
        stream.read_exact(&mut version).await.unwrap();
        stream.write_all(&[1, SEC_NONE]).await.unwrap();
        assert_eq!(stream.read_u8().await.unwrap(), SEC_NONE);
        stream.write_all(&0u32.to_be_bytes()).await.unwrap();
        stream.read_u8().await.unwrap();
        let mut init = [0; 24];
        init[..2].copy_from_slice(&3840u16.to_be_bytes());
        init[2..4].copy_from_slice(&2160u16.to_be_bytes());
        init[4..20].copy_from_slice(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0]);
        stream.write_all(&init).await.unwrap();
        let _ = read_client_setup(&mut stream).await;
        let mut update = vec![0, 0, 0, 1, 0, 0, 0, 0];
        update.extend_from_slice(&3840u16.to_be_bytes());
        update.extend_from_slice(&2160u16.to_be_bytes());
        update.extend_from_slice(&0i32.to_be_bytes());
        update.extend_from_slice(&[1, 2, 3, 0]);
        stream.write_all(&update).await.unwrap();
        partial_tx.send(()).unwrap();

        // Withhold the remaining 33 MB until every down/up has arrived.
        for _ in 0..500 {
            for down in [1, 0] {
                let mut event = [0; 8];
                timeout(Duration::from_secs(2), stream.read_exact(&mut event))
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(event, [4, down, 0, 0, 0, 0, 0, b'a']);
            }
        }
        // Let the caller inspect counters before disconnecting.
        let _ = finish_rx.await;
    });
    let session = connect_vnc(addr, "").await.unwrap();
    partial_rx.await.unwrap();
    for _ in 0..500 {
        for kind in [KeyKind::Down, KeyKind::Up] {
            session
                .cmd_tx
                .try_send(ControlMsg::KeyEvent {
                    vk_code: 0,
                    modifiers: KeyModifiers::empty(),
                    kind,
                    unicode: Some('a'),
                })
                .expect("a transient input burst must not drop a key or disconnect");
        }
    }
    timeout(Duration::from_secs(2), async {
        // Reader and writer are independent tasks. Draining the input writer
        // does not imply the reader has been scheduled for its first bytes.
        while session.cmd_tx.snapshot().sent < 1000 || session.stats.snapshot().received_bytes == 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(session.cmd_tx.snapshot().pending, 0);
    assert_eq!(session.stats.snapshot().received_frames, 0);
    assert!(session.stats.snapshot().received_bytes > 0);
    finish_tx.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn stalled_server_greeting_reports_its_stage_and_closes_socket() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut byte = [0];
        // No greeting: the client must time out and release this connection.
        assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
    });
    let result = timeout(Duration::from_secs(12), connect_vnc(addr, ""))
        .await
        .expect("a stalled greeting must have a bounded timeout");
    assert!(matches!(
        result,
        Err(VncError::Timeout {
            stage: "server greeting",
            seconds: 10
        })
    ));
    timeout(Duration::from_secs(1), server)
        .await
        .unwrap()
        .unwrap();
}

async fn read_client_setup(stream: &mut TcpStream) -> [u8; 10] {
    let mut format = [0; 20];
    stream.read_exact(&mut format).await.unwrap();
    assert_eq!(format[0], 0);
    assert_eq!(&format[4..8], &[32, 24, 0, 1]);
    assert_eq!(stream.read_u8().await.unwrap(), 2);
    stream.read_u8().await.unwrap();
    let count = stream.read_u16().await.unwrap();
    let mut encodings = Vec::new();
    for _ in 0..count {
        encodings.push(stream.read_i32().await.unwrap());
    }
    for supported in [0, 1, 5, -223, -308, -224] {
        assert!(encodings.contains(&supported));
    }
    let mut request = [0; 10];
    stream.read_exact(&mut request).await.unwrap();
    assert_eq!(&request[..2], &[3, 0]);
    request
}

async fn serve_none(stream: &mut TcpStream, banner: &[u8; 12], width: u16, height: u16) {
    stream.write_all(banner).await.unwrap();
    let mut reply = [0; 12];
    stream.read_exact(&mut reply).await.unwrap();
    stream.write_all(&[1, 1]).await.unwrap();
    assert_eq!(stream.read_u8().await.unwrap(), 1);
    stream.write_u32(0).await.unwrap();
    assert_eq!(stream.read_u8().await.unwrap(), 1);
    let mut init = [0; 24];
    init[..2].copy_from_slice(&width.to_be_bytes());
    init[2..4].copy_from_slice(&height.to_be_bytes());
    stream.write_all(&init).await.unwrap();
    read_client_setup(stream).await;
}

fn update_rect(width: u16, height: u16, encoding: i32, payload: &[u8]) -> Vec<u8> {
    let mut update = vec![0, 0, 0, 1, 0, 0, 0, 0];
    update.extend_from_slice(&width.to_be_bytes());
    update.extend_from_slice(&height.to_be_bytes());
    update.extend_from_slice(&encoding.to_be_bytes());
    update.extend_from_slice(payload);
    update
}

#[tokio::test]
async fn apple_empty_updates_and_resize_continue_requesting_pixels() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        serve_none(&mut stream, ARD_VERSION, 0, 0).await;
        stream.write_all(&[0, 0, 0, 0]).await.unwrap();
        let mut request = [0; 10];
        timeout(Duration::from_secs(2), stream.read_exact(&mut request))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(request, [3, 0, 0, 0, 0, 0, 255, 255, 255, 255]);
        // Hextile can also supply the first inferred Apple framebuffer.
        stream
            .write_all(&update_rect(2, 1, 5, &[2, 1, 2, 3, 0]))
            .await
            .unwrap();
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(request, [3, 0, 0, 0, 0, 0, 0, 2, 0, 1]);
        stream
            .write_all(&update_rect(3, 2, -223, &[]))
            .await
            .unwrap();
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(request, [3, 0, 0, 0, 0, 0, 0, 3, 0, 2]);
        stream
            .write_all(&update_rect(3, 2, 5, &[2, 7, 8, 9, 0]))
            .await
            .unwrap();
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(request, [3, 1, 0, 0, 0, 0, 0, 3, 0, 2]);
    });
    let mut session = connect_vnc(addr, "").await.unwrap();
    let final_frame = timeout(Duration::from_secs(3), async {
        loop {
            let frame = session.decoded_bgra_rx.recv().await.unwrap();
            if frame.width == 3 && frame.data == [7, 8, 9, 255].repeat(6) {
                break frame;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(final_frame.height, 2);
    server.await.unwrap();
}

#[tokio::test]
async fn malformed_frame_surfaces_error_and_closes_both_socket_halves() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        serve_none(&mut stream, RFB_VERSION, 1, 1).await;
        stream.write_all(&update_rect(2, 1, 0, &[])).await.unwrap();
        assert_eq!(
            timeout(Duration::from_secs(2), stream.read(&mut [0]))
                .await
                .unwrap()
                .unwrap(),
            0
        );
    });
    let mut session = connect_vnc(addr, "").await.unwrap();
    assert!(
        timeout(Duration::from_secs(2), session.decoded_bgra_rx.recv())
            .await
            .unwrap()
            .is_none()
    );
    let error = (&mut session.completion).await.unwrap().unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    server.await.unwrap();
}

#[tokio::test]
#[ignore = "requires REMOVENT_LIBVNCSERVER_FIXTURE built from tests/fixtures/libvncserver.c"]
async fn independent_libvncserver_interoperability() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let fixture = std::env::var("REMOVENT_LIBVNCSERVER_FIXTURE").expect("fixture executable path");
    for version in [3, 7, 8] {
        for auth in [false, true] {
            timeout(Duration::from_secs(10), async {
                let reserve = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = reserve.local_addr().unwrap();
                drop(reserve);
                let mut child = tokio::process::Command::new(&fixture)
                    .args([
                        addr.port().to_string(),
                        version.to_string(),
                        u8::from(auth).to_string(),
                    ])
                    .kill_on_drop(true)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .unwrap();
                let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
                assert_eq!(output.next_line().await.unwrap().as_deref(), Some("READY"));
                let mut session = connect_vnc(addr, if auth { "interop-password" } else { "" })
                    .await
                    .unwrap();
                let frame = session.decoded_bgra_rx.recv().await.unwrap();
                assert_eq!((frame.width, frame.height), (64, 48));
                let at = (40 * 64 + 40) * 4;
                assert_eq!(&frame.data[at..at + 4], &[40, 40, 123, 255]);
                session
                    .cmd_tx
                    .send(ControlMsg::MouseEvent {
                        display_id: 0,
                        x_px: 12.,
                        y_px: 24.,
                        buttons: 2,
                        kind: MouseKind::RightDown,
                    })
                    .await
                    .unwrap();
                assert_eq!(
                    output.next_line().await.unwrap().as_deref(),
                    Some("POINTER 4 12 24 ENCODING 5")
                );
                let key = |character| ControlMsg::KeyEvent {
                    vk_code: u16::MAX,
                    modifiers: KeyModifiers::empty(),
                    kind: removent_proto::KeyKind::Down,
                    unicode: Some(character),
                };
                session.cmd_tx.send(key('c')).await.unwrap();
                loop {
                    let frame = session.decoded_bgra_rx.recv().await.unwrap();
                    let at = (16 * 64 + 16) * 4;
                    if frame.data[at..at + 4] == [0, 0, 123, 255] {
                        break;
                    }
                }
                session.cmd_tx.send(key('r')).await.unwrap();
                loop {
                    let frame = session.decoded_bgra_rx.recv().await.unwrap();
                    if (frame.width, frame.height) == (80, 60) {
                        let at = (40 * 80 + 40) * 4;
                        if frame.data[at..at + 4] == [40, 40, 123, 255] {
                            break;
                        }
                    }
                }
                drop(session);
                child.kill().await.unwrap();
                child.wait().await.unwrap();
            })
            .await
            .unwrap_or_else(|_| panic!("LibVNCServer RFB 3.{version}, auth={auth} timed out"));
            eprintln!(
                "LibVNCServer RFB 3.{version}, auth={auth}: Hextile, CopyRect, resize, right-click passed"
            );
        }
    }
}

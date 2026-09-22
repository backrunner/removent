use super::*;
use removent_proto::KeyModifiers;

#[test]
fn vnc_password_matches_standard_des_vector() {
    let challenge = *b"0123456789abcdef";
    assert_eq!(
        hex::encode(vnc_response(b"password", &challenge)),
        "5645abeb5f1e6475e8feb11beb66ea19"
    );
    assert_ne!(
        vnc_response(b"password", &challenge),
        vnc_response(b"other", &challenge)
    );
}

#[test]
fn keysyms_cover_common_macos_keys() {
    for (keysym, vk) in [
        (0xff08, 0x33),
        (0xff09, 0x30),
        (0xff0d, 0x24),
        (0xff1b, 0x35),
    ] {
        assert_eq!(key_for_keysym(keysym).unwrap().0, vk);
    }
    assert_eq!(key_for_keysym('a' as u32).unwrap().0, 0x00);
    assert_eq!(key_for_keysym(0xff51).unwrap().0, 0x7b);
    assert_eq!(key_for_keysym(0xffe1).unwrap().1, Some(KeyModifiers::SHIFT));
}

#[test]
fn raw_frame_packet_is_rfb_big_endian_header_and_bgra_payload() {
    let frame = Frame {
        data: vec![1, 2, 3, 4],
        width: 1,
        height: 1,
    };
    let mut packet = Vec::new();
    send_frame_message(&mut packet, &frame, 0, 0, 1, 1, default_pixel_format());
    assert_eq!(
        &packet[..16],
        &[0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0]
    );
    assert_eq!(&packet[16..], &[1, 2, 3, 0]);
}

#[test]
fn raw_frame_honours_client_16_bit_big_endian_format() {
    let frame = Frame {
        data: vec![255, 0, 255, 255],
        width: 1,
        height: 1,
    };
    let format = PixelFormat {
        bits_per_pixel: 16,
        depth: 16,
        big_endian: true,
        true_colour: true,
        red_max: 31,
        green_max: 63,
        blue_max: 31,
        red_shift: 11,
        green_shift: 5,
        blue_shift: 0,
    };
    let mut packet = Vec::new();
    send_frame_message(&mut packet, &frame, 0, 0, 1, 1, format);
    assert_eq!(&packet[16..], &[0xf8, 0x1f]);
}

#[test]
fn raw_frame_supports_eight_bit_true_colour() {
    let format = PixelFormat {
        bits_per_pixel: 8,
        depth: 8,
        red_max: 7,
        green_max: 7,
        blue_max: 3,
        red_shift: 0,
        green_shift: 3,
        blue_shift: 6,
        ..default_pixel_format()
    };
    assert!(format.supported());
    let frame = Frame {
        data: vec![255, 0, 255, 255],
        width: 1,
        height: 1,
    };
    let mut packet = Vec::new();
    send_frame_message(&mut packet, &frame, 0, 0, 1, 1, format);
    assert_eq!(&packet[16..], &[0xc7]);
}

#[tokio::test]
async fn server_handshake_supports_all_standard_versions() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for version in [b"RFB 003.003\n", b"RFB 003.007\n", b"RFB 003.008\n"] {
        for password in ["", "password"] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                protocol::perform_handshake(&mut stream, password.as_bytes())
                    .await
                    .unwrap();
            });
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                let mut stream = TcpStream::connect(addr).await.unwrap();
                let mut greeting = [0; 12];
                stream.read_exact(&mut greeting).await.unwrap();
                stream.write_all(version).await.unwrap();
                let security = if password.is_empty() { 1 } else { 2 };
                if version == b"RFB 003.003\n" {
                    assert_eq!(stream.read_u32().await.unwrap(), security as u32);
                } else {
                    assert_eq!(stream.read_u8().await.unwrap(), 1);
                    assert_eq!(stream.read_u8().await.unwrap(), security);
                    stream.write_u8(security).await.unwrap();
                }
                if security == 2 {
                    let mut challenge = [0; 16];
                    stream.read_exact(&mut challenge).await.unwrap();
                    stream
                        .write_all(&vnc_response(password.as_bytes(), &challenge))
                        .await
                        .unwrap();
                }
                if version == RFB_VERSION || security == 2 {
                    assert_eq!(stream.read_u32().await.unwrap(), 0);
                }
                stream.write_u8(1).await.unwrap();
                let mut init = [0; 24];
                stream.read_exact(&mut init).await.unwrap();
                assert!(u16::from_be_bytes([init[0], init[1]]) > 0);
                assert_eq!(init[4], 32);
                let mut name =
                    vec![0; u32::from_be_bytes(init[20..24].try_into().unwrap()) as usize];
                stream.read_exact(&mut name).await.unwrap();
                assert_eq!(name, b"Removent VNC");
            })
            .await
            .unwrap();
            server.await.unwrap();
        }
    }
}

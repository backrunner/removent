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

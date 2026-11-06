use super::*;
use std::cell::RefCell;

#[test]
fn encoder_surface_satisfies_metal_row_alignment() {
    let (width, height) = (318, 242);
    let mut encoder =
        VideoEncoder::new(removent_proto::CodecId::H264, width, height, 4000, 30).unwrap();
    encoder
        .encode_bgra(&vec![128; width * height * 4], 0)
        .unwrap();
    let surface = encoder.surface.as_ref().unwrap();
    assert_eq!((surface.width(), surface.height()), (width, height));
    assert!(surface.bytes_per_row() >= width * 4);
    // Assert the driver's texture requirement even on machines whose
    // encoder happens not to import this surface through Metal.
    assert_eq!(surface.bytes_per_row() % 16, 0);
}

#[test]
fn native_decode_retains_bgra_surface_after_decoder_shutdown() {
    for codec in [removent_proto::CodecId::H264, removent_proto::CodecId::Hevc] {
        let (width, height) = (318, 242);
        let mut encoder = VideoEncoder::new(codec, width, height, 4000, 60).unwrap();
        let mut pixels = vec![0; width * height * 4];
        for row in 0..height {
            for col in 0..width {
                let color = if col < width / 2 {
                    [20, 40, 220, 255]
                } else {
                    [220, 40, 20, 255]
                };
                pixels[(row * width + col) * 4..][..4].copy_from_slice(&color);
            }
        }
        let packets = encoder.encode_bgra(&pixels, 123_456).unwrap();
        assert!(!packets.is_empty());
        let (tx, rx) = mpsc::channel();
        let decoder = VideoDecoder::with_surface_output(
            codec,
            width,
            height,
            encoder.parameter_sets().unwrap(),
            true,
            move |frame| {
                let _ = tx.send(frame);
            },
        )
        .unwrap();
        for packet in packets {
            decoder.decode_annexb(&packet.data, packet.pts_us).unwrap();
        }
        let output = rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        assert!(
            output.data.is_empty(),
            "native output must not copy CPU pixels"
        );
        assert_eq!(output.pts_us, 123_456);
        drop(decoder);
        drop(encoder);
        let surface = output.surface.unwrap();
        assert_eq!(surface.pixel_format(), BGRA_FOURCC);
        assert_eq!((surface.width(), surface.height()), (width, height));
        let readback = pixel_buffer_to_bgra(&surface).unwrap();
        for (x, red) in [(width / 4, true), (width * 3 / 4, false)] {
            let pixel = &readback[(height / 2 * width + x) * 4..][..4];
            assert!(
                pixel[if red { 2 } else { 0 }] > 180,
                "color/lifetime: {pixel:?}"
            );
            assert!(pixel[if red { 0 } else { 2 }] < 60, "BGRA order: {pixel:?}");
        }
    }
}

#[test]
fn decoder_rejects_parameter_sets_with_different_frame_geometry() {
    for codec in [removent_proto::CodecId::H264, removent_proto::CodecId::Hevc] {
        let (width, height) = (318, 242);
        let mut encoder = VideoEncoder::new(codec, width, height, 4000, 60).unwrap();
        encoder
            .encode_bgra(&vec![128; width * height * 4], 0)
            .unwrap();
        for native in [false, true] {
            // Same byte count but different shape must not bypass validation.
            let decoder = VideoDecoder::with_surface_output(
                codec,
                height,
                width,
                encoder.parameter_sets().unwrap(),
                native,
                |_| {},
            );
            assert!(decoder.is_err(), "{codec:?}, native={native}");
        }
    }
}
/// A force-keyframe attempt that fails must be retried as a regular frame:
/// the frame is encoded, not dropped.
#[test]
fn force_keyframe_failure_retries_without_hint() {
    let calls = RefCell::new(Vec::new());
    let result: Result<i32, VideoError> = encode_maybe_forced(true, |force| {
        calls.borrow_mut().push(force);
        if force {
            Err(VideoError::CoreMedia(-12902)) // kVTPropertyNotSupportedErr
        } else {
            Ok(1)
        }
    });
    assert_eq!(result.unwrap(), 1);
    assert_eq!(*calls.borrow(), vec![true, false]);
}

/// When the regular retry also fails, the error propagates.
#[test]
fn force_keyframe_failure_propagates_when_retry_fails() {
    let result: Result<(), VideoError> =
        encode_maybe_forced(true, |_| Err(VideoError::CoreMedia(-1)));
    assert!(matches!(result, Err(VideoError::CoreMedia(-1))));
}

/// Without a pending keyframe request the hint is never attached.
#[test]
fn no_force_keyframe_single_plain_encode() {
    let calls = RefCell::new(Vec::new());
    let result: Result<i32, VideoError> = encode_maybe_forced(false, |force| {
        calls.borrow_mut().push(force);
        Ok(2)
    });
    assert_eq!(result.unwrap(), 2);
    assert_eq!(*calls.borrow(), vec![false]);
}

#[test]
fn nv12_video_range_maps_neutral_black_and_white() {
    let y = [16, 235];
    let uv = [128, 128];
    let bgra = nv12_to_bgra(
        y.as_ptr(),
        2,
        uv.as_ptr(),
        2,
        2,
        1,
        YuvColor {
            bt709: true,
            full_range: false,
        },
    );
    assert_eq!(bgra, [0, 0, 0, 255, 255, 255, 255, 255]);
}

#[test]
fn nv12_full_range_maps_neutral_black_and_white() {
    let y = [0, 255];
    let uv = [128, 128];
    let bgra = nv12_to_bgra(
        y.as_ptr(),
        2,
        uv.as_ptr(),
        2,
        2,
        1,
        YuvColor {
            bt709: true,
            full_range: true,
        },
    );
    assert_eq!(bgra, [0, 0, 0, 255, 255, 255, 255, 255]);
}

#[test]
fn p010_video_range_maps_neutral_black_and_white_with_padding() {
    let mut y = Vec::new();
    for samples in [[64_u16, 940], [940, 64]] {
        for sample in samples {
            y.extend_from_slice(&(sample << 6).to_le_bytes());
        }
        y.extend_from_slice(&[0, 0]);
    }
    let mut uv = Vec::new();
    for sample in [512_u16, 512] {
        uv.extend_from_slice(&(sample << 6).to_le_bytes());
    }
    uv.extend_from_slice(&[0, 0]);

    let bgra = p010_video_to_bgra(y.as_ptr(), 6, uv.as_ptr(), 6, 2, 2, true);
    assert_eq!(
        bgra,
        [
            0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255,
        ]
    );
}

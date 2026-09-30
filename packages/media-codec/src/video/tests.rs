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

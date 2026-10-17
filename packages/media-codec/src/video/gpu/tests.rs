use super::*;

fn quadrants(width: usize, height: usize) -> CVPixelBuffer {
    let buffer = pixel_buffer(width, height).unwrap();
    let mut guard = buffer.lock_read_write().unwrap();
    let stride = guard.bytes_per_row();
    let base = guard.base_address_mut().unwrap();
    for y in 0..height {
        for x in 0..width {
            let color = [
                if x < width / 2 { 240 } else { 20 },
                if y < height / 2 { 30 } else { 220 },
                80,
                255,
            ];
            // SAFETY: this exclusively owned test buffer is locked, and the
            // write fits its checked pixel row; no GPU/encoder has seen it.
            unsafe {
                std::ptr::copy_nonoverlapping(color.as_ptr(), base.add(y * stride + x * 4), 4);
            }
        }
    }
    drop(guard);
    buffer
}

fn pixel(buffer: &CVPixelBuffer, x: usize, y: usize) -> [u8; 4] {
    let guard = buffer.lock_read_only().unwrap();
    let mut value = [0; 4];
    unsafe {
        std::ptr::copy_nonoverlapping(
            guard.base_address().add(y * guard.bytes_per_row() + x * 4),
            value.as_mut_ptr(),
            4,
        );
    }
    value
}

#[test]
fn metal_lanczos_scales_padded_bgra_without_swapping_channels_or_mutating_source() {
    let source = quadrants(638, 482);
    let mut scaler = MetalScaler::new(318, 242).unwrap();
    let output = scaler.scale(&source).unwrap();
    assert_eq!((output.width(), output.height()), (318, 242));
    for (x, y, expected) in [
        (0, 0, [240, 30, 80, 255]),
        (317, 241, [20, 220, 80, 255]),
        (40, 40, [240, 30, 80, 255]),
        (270, 40, [20, 30, 80, 255]),
        (40, 200, [240, 220, 80, 255]),
        (270, 200, [20, 220, 80, 255]),
    ] {
        let actual = pixel(&output, x, y);
        for c in 0..4 {
            assert!(
                actual[c].abs_diff(expected[c]) <= 1,
                "{actual:?} != {expected:?}"
            );
        }
    }
    assert_eq!(pixel(&source, 40, 40), [240, 30, 80, 255]);
    // Reusing the destination must wait for the previous GPU command.
    drop(output);
    let again = scaler.scale(&source).unwrap();
    assert_eq!(pixel(&again, 270, 200), [20, 220, 80, 255]);
}

#[test]
fn metal_preserves_and_clears_color_tags_on_reused_output() {
    let source = quadrants(320, 240);
    let mut scaler = MetalScaler::new(160, 120).unwrap();
    unsafe {
        apple_cf::raw::CVBufferSetAttachment(
            source.as_ptr().cast(),
            apple_cf::raw::kCVImageBufferYCbCrMatrixKey,
            apple_cf::raw::kCVImageBufferYCbCrMatrix_ITU_R_709_2.cast(),
            1,
        );
    }
    let output = scaler.scale(&source).unwrap();
    assert!(super::super::pixels::is_bt709(&output));
    drop(output);
    unsafe {
        apple_cf::raw::CVBufferRemoveAttachment(
            source.as_ptr().cast(),
            apple_cf::raw::kCVImageBufferYCbCrMatrixKey,
        );
    }
    let output = scaler.scale(&source).unwrap();
    assert!(!super::super::pixels::is_bt709(&output));
}

#[test]
fn native_encoder_keeps_source_alive_and_emits_scaled_decodable_keyframes() {
    let source = quadrants(640, 480);
    for (width, height) in [(640, 480), (318, 242)] {
        let mut encoder =
            VideoEncoder::new(removent_proto::CodecId::Hevc, width, height, 12_000, 60).unwrap();
        let packets = encoder.encode_native(&source, 123_456).unwrap();
        assert_eq!(packets.len(), 1);
        assert!(packets[0].keyframe);
        assert_eq!(packets[0].pts_us, 123_456);
        let decoder = VideoDecoder::new(
            removent_proto::CodecId::Hevc,
            width,
            height,
            encoder.parameter_sets().unwrap(),
        )
        .unwrap();
        decoder
            .decode_annexb(&packets[0].data, packets[0].pts_us)
            .unwrap();
        let started = std::time::Instant::now();
        let decoded = loop {
            if let Some(frame) = decoder.try_recv_decoded() {
                break frame;
            }
            assert!(started.elapsed() < std::time::Duration::from_secs(3));
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        assert_eq!(decoded.data.len(), width * height * 4);
        let offset = ((height / 4) * width + width / 4) * 4;
        for (actual, expected) in decoded.data[offset..offset + 3]
            .iter()
            .zip([240_u8, 30, 80])
        {
            assert!(
                actual.abs_diff(expected) < 15,
                "{width}x{height} decoded {:?}",
                &decoded.data[offset..offset + 4]
            );
        }
        encoder.request_keyframe();
        assert!(encoder.encode_native(&source, 140_123).unwrap()[0].keyframe);
    }
}

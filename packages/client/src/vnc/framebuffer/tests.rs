use super::*;

fn rectangle(x: u16, y: u16, width: u16, height: u16, pixels: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0, 0, 1];
    for value in [x, y, width, height] {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    bytes.extend_from_slice(&0i32.to_be_bytes());
    bytes.extend_from_slice(pixels);
    bytes
}

#[tokio::test]
async fn direct_bgra_reads_preserve_other_rows_and_normalize_padding() {
    let dimensions = Arc::new(RwLock::new(FrameSize {
        width: 4,
        height: 3,
        authoritative: true,
    }));
    let mut pixels = vec![42; 4 * 3 * 4];
    let mut decode_time = Duration::ZERO;
    let bytes = rectangle(
        1,
        1,
        2,
        2,
        &[1, 2, 3, 0, 4, 5, 6, 12, 7, 8, 9, 0, 10, 11, 12, 0],
    );
    assert!(
        read_update(
            &mut bytes.as_slice(),
            &mut pixels,
            &dimensions,
            requested_pixel_format(),
            &mut decode_time
        )
        .await
        .unwrap()
    );
    let mut expected = vec![42; 4 * 3 * 4];
    expected[20..28].copy_from_slice(&[1, 2, 3, 255, 4, 5, 6, 255]);
    expected[36..44].copy_from_slice(&[7, 8, 9, 255, 10, 11, 12, 255]);
    assert_eq!(pixels, expected);
    assert!(
        !read_update(
            &mut [0, 0, 0].as_slice(),
            &mut pixels,
            &dimensions,
            requested_pixel_format(),
            &mut decode_time
        )
        .await
        .unwrap()
    );
    assert_eq!(
        pixels, expected,
        "an empty update must not change or republish the picture"
    );
}

#[tokio::test]
async fn fully_buffered_large_update_yields_for_input() {
    let dimensions = Arc::new(RwLock::new(FrameSize {
        width: 3840,
        height: 2160,
        authoritative: true,
    }));
    let mut pixels = vec![0; 3840 * 2160 * 4];
    let mut decode_time = Duration::ZERO;
    let bytes = rectangle(0, 0, 3840, 2160, &pixels);
    let input_ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let marker = input_ran.clone();
    let input = tokio::spawn(async move {
        marker.store(true, Ordering::Relaxed);
    });
    read_update(
        &mut bytes.as_slice(),
        &mut pixels,
        &dimensions,
        requested_pixel_format(),
        &mut decode_time,
    )
    .await
    .unwrap();
    assert!(
        input_ran.load(Ordering::Relaxed),
        "pixel processing must yield even with all bytes already buffered"
    );
    input.await.unwrap();
    assert!(
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == [0, 0, 0, 255])
    );
}

#[tokio::test]
async fn metadata_only_updates_do_not_republish_or_resize_on_failure() {
    let dimensions = Arc::new(RwLock::new(FrameSize {
        width: 2,
        height: 2,
        authoritative: true,
    }));
    let original = vec![42; 16];
    let mut pixels = original.clone();
    let mut decode_time = Duration::ZERO;
    // LastRect can terminate a nominal 65535-rectangle update.
    let mut last = vec![0, 255, 255];
    last.extend_from_slice(&[0; 8]);
    last.extend_from_slice(&(-224i32).to_be_bytes());
    assert!(
        !read_update(
            &mut last.as_slice(),
            &mut pixels,
            &dimensions,
            requested_pixel_format(),
            &mut decode_time
        )
        .await
        .unwrap()
    );
    // ExtendedDesktopSize: x=1 is a client request reply; y=1 means failure.
    let mut rejected = vec![0, 0, 1, 0, 1, 0, 1, 0, 0, 0, 0];
    rejected.extend_from_slice(&(-308i32).to_be_bytes());
    rejected.extend_from_slice(&[0; 4]);
    assert!(
        !read_update(
            &mut rejected.as_slice(),
            &mut pixels,
            &dimensions,
            requested_pixel_format(),
            &mut decode_time
        )
        .await
        .unwrap()
    );
    assert_eq!(pixels, original);
    assert_eq!(dimensions.read().await.width, 2);
}

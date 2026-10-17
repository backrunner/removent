use super::*;
use removent_media_capture::CapturedVideoFrame;

#[tokio::test]
async fn capture_restart_refreshes_identical_pixels_and_new_dimensions_on_same_stream() {
    let (_endpoints, client, host) = quic_pair().await;
    let (tx, rx) = removent_core::latest::channel();
    let (_kf, kf_rx) = mpsc::channel(1);
    let (_quality, quality_rx) = tokio::sync::watch::channel(QualityState {
        bitrate_kbps: 3000,
        fps: 60,
        scale: 1.,
    });
    let cancel = CancellationToken::new();
    let _guard = cancel.clone().drop_guard();
    let video = spawn_video_loop(
        host.open_media_stream().await.unwrap(),
        rx,
        kf_rx,
        quality_rx,
        CodecId::Hevc,
        320,
        240,
        3000,
        60,
        cancel.clone(),
        None,
        0,
        None,
        None,
    );
    tx.send(CapturedVideoFrame::new(
        [40, 80, 160, 255].repeat(320 * 240),
        320,
        240,
        1,
    ))
    .unwrap();
    let mut stream = client.accept_media_stream().await.unwrap();
    video_packet(&mut stream).await;
    for (generation, (width, height)) in [(320, 240), (240, 320)].into_iter().enumerate() {
        let mut frame =
            CapturedVideoFrame::new([40, 80, 160, 255].repeat(width * height), width, height, 0);
        // Any surviving frame identifies the new capture, even if its first
        // callback was replaced in the latest-frame slot before encoding.
        frame.generation = generation as u64 + 1;
        tx.send(frame).unwrap();
        let (header, _) = video_packet(&mut stream).await;
        assert!(header.is_keyframe());
        assert_ne!(
            header.flags & removent_proto::video_flags::CONFIG_CHANGED,
            0
        );
        assert_eq!(
            (header.width as usize, header.height as usize),
            (width, height)
        );
        assert!(
            header.pts_us > 1,
            "restarted capture timestamps remain monotonic"
        );
        assert!(!cancel.is_cancelled());
    }
    cancel.cancel();
    video.await.unwrap();
}

/// Manual release-mode throughput measurement, independent of CI load. Exercises
/// production pacing, encode, encrypted QUIC, and decode; excludes SCK and GPUI.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual 1080p60 throughput measurement; run with --release --ignored --nocapture"]
async fn lan_video_throughput() {
    let (w, h, fps) = match std::env::var("REMOVENT_QA_VIDEO").as_deref() {
        Ok("4k60") => (3840, 2160, 60),
        Ok("1080p120") => (1920, 1080, 120),
        _ => (1920, 1080, 60),
    };
    let native = std::env::var("REMOVENT_QA_NATIVE").as_deref() == Ok("1");
    let bitrate = removent_proto::VideoQuality::Ultra.bitrate_kbps(w as u32, h as u32, fps);
    let (_endpoints, client, host) = quic_pair().await;
    let (tx, rx) = removent_core::latest::channel();
    let (_kf, kf_rx) = mpsc::channel(1);
    let (_quality, quality_rx) = tokio::sync::watch::channel(QualityState {
        bitrate_kbps: bitrate,
        fps,
        scale: 1.,
    });
    let cancel = CancellationToken::new();
    let _guard = cancel.clone().drop_guard();
    let video = spawn_video_loop(
        host.open_media_stream().await.unwrap(),
        rx,
        kf_rx,
        quality_rx,
        CodecId::Hevc,
        w,
        h,
        bitrate,
        fps,
        cancel.clone(),
        None,
        0,
        None,
        None,
    );
    let started = std::time::Instant::now();
    let producer_stop = cancel.clone();
    let native_frames = native.then(|| {
        (0..4)
            .map(|n| {
                let mut pixels = vec![0; w * h * 4];
                for y in 0..h {
                    let color = [((y + n * 8) % 255) as u8, 100, 180, 255];
                    for pixel in pixels[y * w * 4..(y + 1) * w * 4].as_chunks_mut::<4>().0 {
                        pixel.copy_from_slice(&color);
                    }
                }
                native_buffer(w, h, &pixels)
            })
            .collect::<Vec<_>>()
    });
    let producer = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs_f64(1. / f64::from(fps)));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut n = 0usize;
        loop {
            tokio::select! { _ = producer_stop.cancelled() => break, _ = tick.tick() => {} }
            if let Some(frames) = &native_frames {
                tx.send(CapturedVideoFrame {
                    pixels: removent_media_capture::CapturedPixels::Native(
                        frames[n % frames.len()].clone(),
                    ),
                    width: w,
                    height: h,
                    pts_us: started.elapsed().as_micros() as i64,
                    generation: 0,
                })
                .unwrap();
                n += 1;
                continue;
            }
            let mut pixels = vec![0; w * h * 4];
            // Scrolling bands change the entire image, instead of a tiny marker.
            for y in 0..h {
                let color = [((y + n * 8) % 255) as u8, 100, 180, 255];
                for pixel in pixels[y * w * 4..(y + 1) * w * 4].as_chunks_mut::<4>().0 {
                    pixel.copy_from_slice(&color);
                }
            }
            tx.send(CapturedVideoFrame::new(
                pixels,
                w,
                h,
                started.elapsed().as_micros() as i64,
            ))
            .unwrap();
            n += 1;
        }
    });
    let mut stream = client.accept_media_stream().await.unwrap();
    let mut decoder = None;
    let mut count = 0usize;
    let mut latencies = Vec::new();
    let mut bytes = 0usize;
    let mut previous = None;
    let mut gaps = Vec::new();
    while started.elapsed() < Duration::from_secs(12) {
        let (header, data) = video_packet(&mut stream).await;
        let decoder = decoder.get_or_insert_with(|| {
            removent_media_codec::VideoDecoder::new(
                CodecId::Hevc,
                w,
                h,
                &removent_media_codec::extract_param_sets(&data, true),
            )
            .unwrap()
        });
        decoder.decode_annexb(&data, header.pts_us).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            if let Some(frame) = decoder.try_recv_decoded() {
                if started.elapsed() >= Duration::from_secs(2) {
                    count += 1;
                    bytes += data.len();
                    let now = started.elapsed().as_secs_f64() * 1000.;
                    latencies.push(now - frame.pts_us as f64 / 1000.);
                    if let Some(last) = previous {
                        gaps.push(now - last);
                    }
                    previous = Some(now);
                }
                break;
            }
            assert!(std::time::Instant::now() < deadline, "decoder timed out");
            tokio::time::sleep(Duration::from_micros(250)).await;
        }
    }
    let seconds = started.elapsed().as_secs_f64() - 2.;
    latencies.sort_by(f64::total_cmp);
    gaps.sort_by(f64::total_cmp);
    println!(
        "LAN_VIDEO native={native} size={w}x{h} target_fps={fps} fps={:.2} frames={} latency_p50_ms={:.2} latency_p95_ms={:.2} gap_p95_ms={:.2} mbps={:.2}",
        count as f64 / seconds,
        count,
        latencies[latencies.len() / 2],
        latencies[latencies.len() * 95 / 100],
        gaps[gaps.len() * 95 / 100],
        bytes as f64 * 8. / seconds / 1e6
    );
    cancel.cancel();
    producer.await.unwrap();
    video.await.unwrap();
    assert!(
        count as f64 / seconds >= f64::from(fps) * 0.9,
        "video throughput below 90% of the requested frame rate"
    );
}

/// IOSurface-backed fixture, filled before it is shared with the encoder.
pub(super) fn native_buffer(w: usize, h: usize, pixels: &[u8]) -> apple_cf::cv::CVPixelBuffer {
    let stride = (w * 4).next_multiple_of(64);
    let surface = apple_cf::iosurface::IOSurface::create_with_properties(
        w,
        h,
        u32::from_be_bytes(*b"BGRA"),
        4,
        stride,
        stride * h,
        None,
    )
    .unwrap();
    {
        let mut guard = surface.lock_read_write().unwrap();
        let base = guard.base_address_mut().unwrap();
        for row in 0..h {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    pixels.as_ptr().add(row * w * 4),
                    base.add(row * stride),
                    w * 4,
                );
            }
        }
    }
    apple_cf::cv::CVPixelBuffer::create_with_io_surface(&surface).unwrap()
}

use super::*;

#[tokio::test]
async fn uhd_to_540p_and_back_decodes_without_replacing_the_stream() {
    dynamic_resolution(false).await;
}

#[tokio::test]
async fn native_uhd_to_540p_and_back_decodes_without_cpu_readback() {
    dynamic_resolution(true).await;
}

async fn dynamic_resolution(native: bool) {
    let (_endpoints, client, host) = quic_pair().await;
    let (tx, rx) = removent_core::latest::channel();
    let (_kf, kf_rx) = mpsc::channel(1);
    let maximum = QualityState {
        bitrate_kbps: 100_000,
        fps: 120,
        scale: 1.,
    };
    let (quality, quality_rx) = tokio::sync::watch::channel(maximum);
    let cancel = CancellationToken::new();
    let _guard = cancel.clone().drop_guard();
    let video = spawn_video_loop(
        host.open_media_stream().await.unwrap(),
        rx,
        kf_rx,
        quality_rx,
        CodecId::Hevc,
        3840,
        2160,
        100_000,
        120,
        cancel.clone(),
        None,
        0,
        None,
        None,
    );
    let mut frame = removent_media_capture::CapturedVideoFrame::new(
        [40, 90, 160, 255].repeat(3840 * 2160),
        3840,
        2160,
        1,
    );
    if native {
        frame.pixels = removent_media_capture::CapturedPixels::Native(
            super::capture::native_buffer(3840, 2160, &frame.pixels.bgra(3840, 2160).unwrap()),
        );
    }
    tx.send(frame).unwrap();
    let mut stream = client.accept_media_stream().await.unwrap();
    let mut previous_pts = 0;
    for (index, (w, h, scale)) in [(3840, 2160, 1.), (960, 540, 0.25), (3840, 2160, 1.)]
        .into_iter()
        .enumerate()
    {
        if index != 0 {
            quality.send_replace(QualityState { scale, ..maximum });
        }
        let (header, payload) = video_packet(&mut stream).await;
        assert_eq!((header.width, header.height), (w, h));
        assert!(header.is_keyframe() && header.config_changed());
        assert!(header.pts_us > previous_pts);
        previous_pts = header.pts_us;
        let decoder = removent_media_codec::VideoDecoder::new(
            CodecId::Hevc,
            w as usize,
            h as usize,
            &removent_media_codec::extract_param_sets(&payload, true),
        )
        .unwrap();
        decoder.decode_annexb(&payload, header.pts_us).unwrap();
        let decoded = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(frame) = decoder.try_recv_decoded() {
                    break frame;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(decoded.data.len(), w as usize * h as usize * 4);
        assert!(
            decoded.data.as_chunks::<4>().0.iter().all(|p| p[2] > 140),
            "{w}x{h} first={:?} min_red={}",
            &decoded.data[..4],
            decoded
                .data
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| p[2])
                .min()
                .unwrap()
        );
        assert!(!cancel.is_cancelled());
    }
    cancel.cancel();
    video.await.unwrap();
}

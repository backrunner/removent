use super::*;
use crate::session::tests::quic_pair;

#[tokio::test]
async fn audio_capture_eof_does_not_cancel_video() {
    let (_endpoints, client, host) = quic_pair().await;
    let cancel = CancellationToken::new();
    let _stop_on_drop = cancel.clone().drop_guard();
    let (audio_tx, audio_rx) = mpsc::channel(1);
    let audio = spawn_audio_loop(
        host.open_media_stream().await.unwrap(),
        audio_rx,
        96,
        cancel.clone(),
    );
    drop(audio_tx);
    tokio::time::timeout(Duration::from_secs(2), audio)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !cancel.is_cancelled(),
        "optional audio must not end the session"
    );

    let (frames, rx) = removent_core::latest::channel();
    let (_kf, kf_rx) = mpsc::channel(1);
    let (_quality, quality_rx) = tokio::sync::watch::channel(
        AdaptationController::with_dimensions(
            3000,
            30,
            removent_core::QualityPreset::Auto,
            320,
            240,
        )
        .state(),
    );
    let video = spawn_video_loop(
        host.open_media_stream().await.unwrap(),
        rx,
        kf_rx,
        quality_rx,
        CodecId::H264,
        320,
        240,
        3000,
        30,
        cancel.clone(),
        None,
        0,
        None,
        None,
    );
    frames
        .send(removent_media_capture::CapturedVideoFrame::new(
            [40, 80, 160, 255].repeat(320 * 240),
            320,
            240,
            1,
        ))
        .unwrap();
    // The unused audio stream is finished without bytes; discard its EOF.
    let header = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let mut stream = client.accept_media_stream().await.unwrap();
            let mut bytes = [0; 27];
            if stream.read_exact(&mut bytes).await.is_ok() {
                break removent_proto::parse_video_header(&bytes).unwrap().0;
            }
        }
    })
    .await
    .unwrap();
    assert!(header.is_keyframe());
    assert_eq!((header.width, header.height), (320, 240));
    cancel.cancel();
    video.await.unwrap();
}

#[tokio::test]
async fn blocked_audio_write_exits_on_cancel() {
    let (_endpoints, _client, host) = quic_pair().await;
    host.inner().set_send_window(0);
    let stream = host.open_media_stream().await.unwrap();
    let (tx, rx) = mpsc::channel(1);
    let cancel = CancellationToken::new();
    let mut task = spawn_audio_loop(stream, rx, 96, cancel.clone());
    tx.send(removent_media_capture::AudioFrame {
        samples: vec![0; 960],
        pts_micros: 1,
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while tx.capacity() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!task.is_finished());
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(1), &mut task)
        .await
        .unwrap()
        .unwrap();
}

use super::*;
use crate::{RecordedInput, RecorderInputSink};
use removent_proto::KeyModifiers;
mod native;

async fn pump_with_input(
    client: &RvpConnection,
    host: &RvpConnection,
    input: Arc<dyn InputSink>,
) -> (
    ControlSink,
    tokio::task::JoinHandle<()>,
    (mpsc::Sender<ControlMsg>, removent_net::quinn::RecvStream),
) {
    let (send, recv_client) = client.inner().open_bi().await.unwrap();
    let mut sink = ControlSink::new(send, removent_net::ControlCodec);
    sink.send(ControlMsg::Ping { ts_us: 0 }).await.unwrap();
    let (send, recv) = host.inner().accept_bi().await.unwrap();
    let (tx, rx) = mpsc::channel(4);
    let task = spawn_control_pump(
        ControlSource::new(recv, removent_net::ControlCodec),
        ControlSink::new(send, removent_net::ControlCodec),
        ControlPumpDeps {
            automation: None,
            conn: host.clone(),
            kf_tx: None,
            controller: None,
            window_ms: 250,
            input: Some(input),
            local_clip: None,
            quality_tx: None,
            caps: Caps::all(),
            clip_state: None,
            cancel: CancellationToken::new(),
            peer_fp: None,
            delivery: None,
        },
        rx,
    );
    (sink, task, (tx, recv_client))
}

fn mouse(kind: MouseKind, buttons: u8) -> ControlMsg {
    ControlMsg::MouseEvent {
        display_id: 0,
        x_px: 100.,
        y_px: 100.,
        buttons,
        kind,
    }
}

#[tokio::test]
async fn motion_pressure_preserves_clicks_typing_and_teardown_releases() {
    let (_endpoints, client, host) = quic_pair().await;
    let recorder = Arc::new(RecorderInputSink::default());
    let (mut sink, task, _keep) = pump_with_input(&client, &host, recorder.clone()).await;
    // Exhaust the continuous-event budget before sending a double click.
    for _ in 0..1200 {
        sink.send(mouse(MouseKind::Moved, 0)).await.unwrap();
    }
    for _ in 0..2 {
        sink.send(mouse(MouseKind::LeftDown, 1)).await.unwrap();
        sink.send(mouse(MouseKind::LeftUp, 0)).await.unwrap();
    }
    sink.send(ControlMsg::KeyEvent {
        vk_code: 0,
        modifiers: KeyModifiers::empty(),
        kind: KeyKind::Down,
        unicode: Some('a'),
    })
    .await
    .unwrap();
    sink.send(ControlMsg::SessionEnd {
        reason: removent_proto::EndReason::ClientClosed,
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    let events = recorder.events.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(
                e,
                RecordedInput::Mouse {
                    kind: MouseKind::LeftDown,
                    ..
                }
            ))
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(
                e,
                RecordedInput::Mouse {
                    kind: MouseKind::LeftUp,
                    ..
                }
            ))
            .count(),
        2
    );
    assert!(events.iter().any(|e| matches!(
        e,
        RecordedInput::Key {
            kind: KeyKind::Down,
            ..
        }
    )));
    assert!(matches!(
        events.last(),
        Some(RecordedInput::Key {
            kind: KeyKind::Up,
            ..
        })
    ));
}

#[tokio::test]
async fn disconnect_ends_scrolling_and_releases_all_three_buttons_and_modifiers() {
    let (_endpoints, client, host) = quic_pair().await;
    let recorder = Arc::new(RecorderInputSink::default());
    let (mut sink, task, _keep) = pump_with_input(&client, &host, recorder.clone()).await;
    sink.send(ControlMsg::ScrollEvent {
        display_id: 0,
        dx_mm: 0.,
        dy_mm: 1.,
        phase: ScrollPhase::Began,
    })
    .await
    .unwrap();
    for (kind, buttons) in [
        (MouseKind::LeftDown, 1),
        (MouseKind::RightDown, 3),
        (MouseKind::MiddleDown, 7),
    ] {
        sink.send(mouse(kind, buttons)).await.unwrap();
    }
    sink.send(ControlMsg::KeyEvent {
        vk_code: 0x37,
        modifiers: KeyModifiers::COMMAND,
        kind: KeyKind::FlagsChanged,
        unicode: None,
    })
    .await
    .unwrap();
    // Abrupt transport closure, without SessionEnd or any releases.
    tokio::time::timeout(Duration::from_secs(3), async {
        while recorder.events.lock().unwrap().len() < 5 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    client.inner().close(0u32.into(), b"test network loss");
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    let events = recorder.events.lock().unwrap();
    assert_eq!(events.len(), 10);
    assert!(events.iter().any(|e| matches!(
        e,
        RecordedInput::Scroll {
            phase: ScrollPhase::Ended,
            dx: 0.,
            dy: 0.,
            ..
        }
    )));
    for expected in [MouseKind::LeftUp, MouseKind::RightUp, MouseKind::MiddleUp] {
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RecordedInput::Mouse { kind, .. } if *kind == expected))
        );
    }
    assert!(events.iter().any(|e| matches!(e, RecordedInput::Key { kind: KeyKind::FlagsChanged, mods, .. } if mods.is_empty())));
}

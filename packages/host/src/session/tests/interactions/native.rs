use super::*;

/// Opens a temporary AppKit window and posts real HID events. Run explicitly
/// on an unlocked Mac with Accessibility permission, never in ordinary CI.
#[tokio::test]
#[ignore = "requires an unlocked macOS GUI and Accessibility permission"]
async fn native_quic_events_are_recognized_by_appkit() {
    assert!(removent_input::accessibility_trusted());
    let dir = tempfile::tempdir().unwrap();
    let receiver = dir.path().join("receiver");
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/qa/native_input_receiver.swift");
    let compiled = std::process::Command::new("xcrun")
        .arg("swiftc")
        .arg(source)
        .arg("-o")
        .arg(&receiver)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let log = dir.path().join("events.jsonl");
    let ready = dir.path().join("ready.json");
    let mut process = std::process::Command::new(receiver)
        .arg(&log)
        .arg(&ready)
        .spawn()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let point: serde_json::Value = serde_json::from_slice(&std::fs::read(ready).unwrap()).unwrap();
    let g = removent_input::DisplayGeometry::main().unwrap();
    let x = ((point["x"].as_f64().unwrap() - g.origin_x) * g.scale) as f32;
    let y = ((point["y"].as_f64().unwrap() - g.origin_y) * g.scale) as f32;
    let (_endpoints, client, host) = quic_pair().await;
    let input = Arc::new(crate::input_sink::RealInputSink::new());
    let (mut sink, task, _keep) = pump_with_input(&client, &host, input).await;
    let pointer = |kind, buttons| ControlMsg::MouseEvent {
        display_id: u64::from(g.id),
        x_px: x,
        y_px: y,
        buttons,
        kind,
    };
    for _ in 0..2 {
        sink.send(pointer(MouseKind::LeftDown, 1)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        sink.send(pointer(MouseKind::LeftUp, 0)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    // Shift-click must carry the remote session's modifier state.
    sink.send(ControlMsg::KeyEvent {
        vk_code: 0x38,
        modifiers: KeyModifiers::SHIFT,
        kind: KeyKind::FlagsChanged,
        unicode: None,
    })
    .await
    .unwrap();
    sink.send(pointer(MouseKind::RightDown, 2)).await.unwrap();
    sink.send(pointer(MouseKind::RightUp, 0)).await.unwrap();
    sink.send(ControlMsg::KeyEvent {
        vk_code: 0x38,
        modifiers: KeyModifiers::empty(),
        kind: KeyKind::FlagsChanged,
        unicode: None,
    })
    .await
    .unwrap();
    sink.send(pointer(MouseKind::MiddleDown, 4)).await.unwrap();
    sink.send(pointer(MouseKind::MiddleDragged, 4))
        .await
        .unwrap();
    sink.send(pointer(MouseKind::MiddleUp, 0)).await.unwrap();
    for kind in [KeyKind::Down, KeyKind::Down, KeyKind::Up] {
        sink.send(ControlMsg::KeyEvent {
            vk_code: 0,
            modifiers: KeyModifiers::empty(),
            kind,
            unicode: Some('a'),
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    for phase in [ScrollPhase::Began, ScrollPhase::Changed, ScrollPhase::Ended] {
        sink.send(ControlMsg::ScrollEvent {
            display_id: u64::from(g.id),
            dx_mm: 0.,
            dy_mm: if phase == ScrollPhase::Ended { 0. } else { 1. },
            phase,
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    sink.send(ControlMsg::SessionEnd {
        reason: removent_proto::EndReason::ClientClosed,
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let events: Vec<serde_json::Value> = std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    // Let the fixture restore the original foreground app before assertions.
    let status = tokio::task::spawn_blocking(move || process.wait().unwrap())
        .await
        .unwrap();
    assert!(status.success());
    let of_type = |ty: u64| {
        events
            .iter()
            .filter(move |event| event["type"].as_u64() == Some(ty))
    };
    assert_eq!(
        of_type(1)
            .map(|e| e["clicks"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![1, 2],
        "{events:?}"
    );
    assert_eq!(
        of_type(2)
            .map(|e| e["clicks"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(
        of_type(3).any(|e| e["flags"].as_u64().unwrap() & (1 << 17) != 0),
        "Shift-right-click missing: {events:?}"
    );
    for ty in [4, 25, 26, 27, 12] {
        assert!(
            of_type(ty).next().is_some(),
            "missing AppKit type {ty}: {events:?}"
        );
    }
    assert_eq!(
        of_type(10)
            .map(|e| e["repeat"].as_bool().unwrap())
            .collect::<Vec<_>>(),
        vec![false, true],
        "{events:?}"
    );
    assert!(of_type(11).any(|e| e["key"] == 0));
    let phases: Vec<_> = of_type(22).map(|e| e["phase"].as_u64().unwrap()).collect();
    assert!(
        phases.contains(&1) && phases.contains(&4) && phases.contains(&8),
        "{phases:?}"
    );
    eprintln!(
        "AppKit received native QUIC double clicks, Shift-right-click, middle drag, typing/repeat, and scroll phases ({} events)",
        events.len()
    );
}

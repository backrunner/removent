use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_local_takeover_cancels_agent_and_viewer_release_cannot_clear_local_hold() {
    let f = fixture(true).await;
    let conn = f.session.conn.clone();
    let held = tokio::spawn(async move {
        removent_net::automation::call(
            &conn,
            &request(
                "held-locally",
                Command::PressKey {
                    key: "a".into(),
                    hold_ms: 3000,
                },
            ),
        )
        .await
    });
    wait_for(|| f.held.load(Ordering::SeqCst)).await;
    let executor = f.executor.as_ref().unwrap();
    executor.local_input(true);
    assert_eq!(
        held.await.unwrap().unwrap().result.unwrap_err().code,
        "control_changed"
    );
    assert!(!f.held.load(Ordering::SeqCst));
    f.session
        .send_input_key(
            0,
            removent_proto::KeyModifiers::empty(),
            removent_proto::KeyKind::Up,
            None,
        )
        .await
        .unwrap();
    wait_for(|| !f.input.recorder.events.lock().unwrap().is_empty()).await;
    let status = removent_net::automation::call(
        &f.session.conn,
        &request("local-status", Command::ControlStatus {}),
    )
    .await
    .unwrap()
    .result
    .unwrap();
    assert_eq!(status["local_input_held"], true);
    assert_eq!(status["viewer_input_held"], false);
    assert_eq!(status["input_held"], true);
    let busy = removent_net::automation::call(
        &f.session.conn,
        &request(
            "local-busy",
            Command::ResumeControl {
                revision: status["revision"].as_u64().unwrap(),
            },
        ),
    )
    .await
    .unwrap();
    assert_eq!(busy.result.unwrap_err().code, "manual_busy");
    executor.local_input(false);
    tokio::time::sleep(Duration::from_millis(510)).await;
    let status = removent_net::automation::call(
        &f.session.conn,
        &request("released", Command::ControlStatus {}),
    )
    .await
    .unwrap()
    .result
    .unwrap();
    assert_eq!(status["last_input_source"], "local");
    assert_eq!(status["can_resume"], true);
    let resumed = removent_net::automation::call(
        &f.session.conn,
        &request(
            "local-resume",
            Command::ResumeControl {
                revision: status["revision"].as_u64().unwrap(),
            },
        ),
    )
    .await
    .unwrap()
    .result
    .unwrap();
    let late = removent_net::automation::call(
        &f.session.conn,
        &request("local-late", Command::TypeText { text: "no".into() }),
    )
    .await
    .unwrap();
    assert_eq!(late.result.unwrap_err().code, "control_changed");
    let mut fresh = request("local-fresh", Command::TypeText { text: "yes".into() });
    fresh.control_revision = resumed["revision"].as_u64();
    removent_net::automation::call(&f.session.conn, &fresh)
        .await
        .unwrap()
        .result
        .unwrap();
    assert_eq!(f.executed.load(Ordering::SeqCst), 1);
}

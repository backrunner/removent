use super::*;

async fn status_when(f: &Fixture, condition: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let status = rpc(f, Command::ControlStatus {}, None).await.unwrap();
            if condition(&status) {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an unlocked macOS GUI, Accessibility, Input Monitoring and Screen Recording permissions"]
async fn native_automation_rvp_local_os_tap_preserves_external_hold() {
    let receiver = Receiver::start().await;
    let f = fixture_with_native(true, true).await;
    native_ready(&f).await;
    act(
        &f,
        &receiver,
        Command::PressKey {
            key: "a".into(),
            hold_ms: 0,
        },
    )
    .await;
    wait_for(|| {
        receiver
            .events()
            .iter()
            .any(|e| e["type"] == 11 && e["key"] == 0)
    })
    .await;
    let before = receiver.events().len();
    let conn = f.session.conn.clone();
    let held = tokio::spawn(async move {
        removent_net::automation::call(
            &conn,
            &request(
                "local-held",
                Command::PressKey {
                    key: "a".into(),
                    hold_ms: 3000,
                },
            ),
        )
        .await
    });
    wait_for(|| {
        receiver
            .events()
            .iter()
            .skip(before)
            .any(|e| e["type"] == 10 && e["key"] == 0)
    })
    .await;
    receiver.command("key_down").await;
    assert_eq!(
        held.await.unwrap().unwrap().result.unwrap_err().code,
        "control_changed"
    );
    let status = status_when(&f, |s| s["local_input_held"] == true).await;
    assert_eq!(status["last_input_source"], "local");
    assert_eq!(status["local_input_monitor"]["state"], "ready");
    // The cancelled agent must not release the same key held by the external source.
    assert!(
        !receiver
            .events()
            .iter()
            .skip(before)
            .any(|e| e["type"] == 11 && e["key"] == 0),
        "Unexpected key-up during external hold: {:?}",
        receiver
            .events()
            .iter()
            .skip(before)
            .map(|e| (&e["type"], &e["key"]))
            .collect::<Vec<_>>()
    );
    f.session
        .send_input_key(
            1,
            removent_proto::KeyModifiers::empty(),
            removent_proto::KeyKind::Up,
            None,
        )
        .await
        .unwrap();
    wait_for(|| !f.input.recorder.events.lock().unwrap().is_empty()).await;
    let status = rpc(&f, Command::ControlStatus {}, None).await.unwrap();
    assert_eq!(status["local_input_held"], true);
    assert_eq!(
        rpc(
            &f,
            Command::ResumeControl {
                revision: status["revision"].as_u64().unwrap()
            },
            None
        )
        .await
        .unwrap_err()
        .code,
        "manual_busy"
    );
    receiver.command("key_up").await;
    let status = status_when(&f, |s| s["can_resume"] == true).await;
    let resumed = rpc(
        &f,
        Command::ResumeControl {
            revision: status["revision"].as_u64().unwrap(),
        },
        None,
    )
    .await
    .unwrap();
    rpc(
        &f,
        Command::PressKey {
            key: "a".into(),
            hold_ms: 0,
        },
        resumed["revision"].as_u64(),
    )
    .await
    .unwrap();
    eprintln!(
        "Native OS tap acceptance: unmarked external event cancels the agent, preserves the held key, and requires explicit resume; not a physical HID test"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an unlocked macOS GUI, Accessibility and Input Monitoring permissions"]
async fn native_automation_rvp_secure_input_revokes_session() {
    let receiver = Receiver::start().await;
    let f = fixture_with_native(true, true).await;
    native_ready(&f).await;
    receiver.command("secure_on").await;
    let status = status_when(&f, |s| s["local_input_monitor"]["state"] == "unavailable").await;
    assert_eq!(status["local_input_monitor"]["reason"], "secure_input");
    let revision = status["revision"].as_u64();
    assert_eq!(
        rpc(
            &f,
            Command::TypeText {
                text: "must not run".into()
            },
            revision
        )
        .await
        .unwrap_err()
        .code,
        "local_monitor_unavailable"
    );
    assert_eq!(
        rpc(
            &f,
            Command::ResumeControl {
                revision: revision.unwrap()
            },
            None
        )
        .await
        .unwrap_err()
        .code,
        "local_monitor_unavailable"
    );
    rpc(&f, Command::State {}, None).await.unwrap();
    receiver.command("secure_off").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let status = rpc(&f, Command::ControlStatus {}, None).await.unwrap();
    assert_eq!(status["local_input_monitor"]["reason"], "secure_input");
    assert_eq!(status["can_resume"], false);
    eprintln!(
        "Native secure-input acceptance: OS transition revokes mutation and resume, retains reads, and never silently re-enables the session"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an unlocked macOS GUI, Accessibility and Input Monitoring permissions"]
async fn native_automation_rvp_application_lifecycle_refreshes_without_main_runloop() {
    let f = fixture_with_native(true, true).await;
    native_ready(&f).await;
    // Populate any OS caches before the application launches.
    rpc(&f, Command::State {}, None).await.unwrap();
    for _ in 0..2 {
        let receiver = Receiver::start().await;
        let target = receiver.target();
        let state = rpc(&f, Command::State {}, None).await.unwrap();
        assert!(
            state["apps"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["target"] == target && a["active"] == true)
        );
        let tree = rpc(
            &f,
            Command::Snapshot {
                target: "frontmost".into(),
            },
            None,
        )
        .await
        .unwrap();
        assert_eq!(tree["target"], target);
        let reference = element(&tree, "automation-text")["reference"]
            .as_str()
            .unwrap()
            .to_owned();
        drop(receiver);
        let state = rpc(&f, Command::State {}, None).await.unwrap();
        assert!(
            !state["apps"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a["target"] == target)
        );
        let status = rpc(&f, Command::ControlStatus {}, None).await.unwrap();
        assert_eq!(
            rpc(
                &f,
                Command::SetValue {
                    reference,
                    value: "never".into()
                },
                status["revision"].as_u64()
            )
            .await
            .unwrap_err()
            .code,
            "stale_reference"
        );
    }
    eprintln!(
        "Native application lifecycle acceptance: launch, frontmost, exit and stale references refresh without a main AppKit run loop"
    );
}

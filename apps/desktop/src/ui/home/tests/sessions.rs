use super::*;

#[gpui::test]
fn multiple_viewers_survive_new_connections_and_old_events(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let directory = tempfile::tempdir().unwrap();
    let (commands, _rx) = tokio::sync::mpsc::channel(8);
    let engine = Engine::for_ui_test(
        removent_core::DataPaths {
            root: directory.path().into(),
        },
        commands,
    );
    let slot = Rc::new(std::cell::RefCell::new(None));
    let out = slot.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let home = cx.new(|cx| HomeView::new(engine, window, cx));
        *out.borrow_mut() = Some(home.clone());
        gpui_component::Root::new(AnyView::from(home), window, cx)
    });
    let home = slot.borrow_mut().take().unwrap();
    let mut clients = Vec::new();
    let mut frames = Vec::new();
    let mut receivers = Vec::new();
    let mut viewers = Vec::new();
    for (name, address) in [("Office", "192.0.2.1:48688"), ("Studio", "192.0.2.2:48688")] {
        cx.update(|window, cx| {
            home.update(cx, |home, cx| {
                let windows = cx.windows();
                home.start_connect(name.into(), address.parse().unwrap(), cx);
                assert!(home.connecting.is_some());
                let (commands, receiver) = tokio::sync::mpsc::channel(8);
                frames.push(home.engine.publish_test_frames(commands));
                receivers.push(receiver);
                clients.push(home.engine.clone());
                home.handle_event(
                    UiEvent::SessionReady {
                        generation: home.engine.client_generation(),
                        codec: "test".into(),
                    },
                    window,
                    cx,
                );
                assert!(home.connecting.is_none());
                let added: Vec<_> = cx
                    .windows()
                    .into_iter()
                    .filter(|handle| !windows.contains(handle))
                    .collect();
                assert_eq!(added.len(), 1);
                viewers.push(added[0]);
            })
        });
        cx.run_until_parked();
    }
    cx.update(|window, cx| {
        home.update(cx, |home, cx| {
            home.open_connection_dialog(None, window, cx);
            let dialog = home.connection_dialog.clone();
            home.handle_event(
                UiEvent::SessionReady {
                    generation: home.engine.client_generation(),
                    codec: "reconnected".into(),
                },
                window,
                cx,
            );
            assert_eq!(
                home.connection_dialog, dialog,
                "recovery must preserve the new form"
            );
            home.connection_dialog = None;
            home.start_connect("Third".into(), "192.0.2.3:48688".parse().unwrap(), cx);
            for client in &clients {
                home.handle_event(
                    UiEvent::ConnectFailed {
                        generation: client.client_generation(),
                        error: "old viewer ended".into(),
                    },
                    window,
                    cx,
                );
            }
            assert_eq!(home.connecting.as_deref(), Some("Third"));
            home.cancel_connection(cx);
            assert!(
                home.engine.session_active(),
                "other viewers still block updates"
            );
        })
    });
    for client in &clients {
        assert_eq!(
            client.client_recovery().state(),
            removent_client::recovery::State::Connected
        );
    }
    cx.update(|_, cx| {
        viewers[0]
            .update(cx, |_, window, _| window.remove_window())
            .unwrap()
    });
    cx.run_until_parked();
    assert_ne!(
        clients[0].client_generation(),
        clients[1].client_generation()
    );
    assert!(clients[0].take_client_frames().is_none());
    frames[1]
        .send(crate::engine::VideoFrame {
            width: 2,
            height: 2,
            data: vec![80; 16],
            pts_us: 0,
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        clients[1].client_recovery().state(),
        removent_client::recovery::State::Connected
    );
    assert!(clients[1].client_input_ready(clients[1].client_generation()));
    cx.update(|_, cx| {
        viewers[1]
            .update(cx, |_, window, _| window.remove_window())
            .unwrap()
    });
    cx.run_until_parked();
    assert!(!clients[1].session_active());
}

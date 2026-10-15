use super::*;

fn engine() -> (tempfile::TempDir, Engine) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::with_state(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap(),
        DataPaths {
            root: dir.path().into(),
        },
        Settings::default(),
    );
    (dir, engine)
}

fn request(host: &str, protocol: ConnectionProtocol) -> ConnectionRequest {
    let mut request = ConnectionRequest::native("192.0.2.1:48688".parse().unwrap());
    request.protocol = protocol;
    request.address = ConnectionAddress::parse(host, &protocol.default_port().to_string()).unwrap();
    request
}

fn attempt(engine: &Engine) -> ClientAttempt {
    ClientAttempt {
        generation: engine.client_generation(),
        started: std::time::Instant::now(),
        recovery: engine.client_recovery(),
        channels: engine.client_channels.clone(),
        events: engine.events_tx.clone(),
        sessions: Arc::downgrade(&engine.client_sessions),
    }
}

#[test]
fn reservations_allow_other_protocols_and_hosts_but_not_ports_or_credentials() {
    let (_dir, root) = engine();
    let mut clients = Vec::new();
    for protocol in ConnectionProtocol::ALL {
        clients.push(
            root.connect_request(request("office.local", protocol))
                .unwrap(),
        );
        clients.push(
            root.connect_request(request("other.local", protocol))
                .unwrap(),
        );
        let mut duplicate = request("OFFICE.local.", protocol);
        duplicate.address.port += 1;
        duplicate.username = "another-user".into();
        duplicate.password = "another-password".into();
        assert!(root.connect_request(duplicate).is_err());
    }
    assert_eq!(root.client_sessions.lock().unwrap().len(), 6);
    assert!(root.session_active());
    for client in clients {
        client.disconnect_client();
    }
    assert!(!root.session_active());
}

#[test]
fn duplicate_requests_racing_from_clones_reserve_only_one_slot() {
    let (_dir, root) = engine();
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let engine = root.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                engine.connect_request(request("office.local", ConnectionProtocol::Vnc))
            })
        })
        .collect();
    let clients: Vec<_> = threads
        .into_iter()
        .filter_map(|t| t.join().unwrap().ok())
        .collect();
    assert_eq!(clients.len(), 1);
    clients[0].disconnect_client();
}

#[test]
fn ipv6_aliases_and_loopback_addresses_cannot_bypass_reservations() {
    let (_dir, root) = engine();
    for (host, alias) in [
        ("192.0.2.10", "::ffff:192.0.2.10"),
        ("2001:db8::10", "2001:db8:0:0::10"),
        ("127.0.0.1", "::1"),
    ] {
        let client = root
            .connect_request(request(host, ConnectionProtocol::Vnc))
            .unwrap();
        assert!(
            root.connect_request(request(alias, ConnectionProtocol::Vnc))
                .is_err()
        );
        client.disconnect_client();
    }
}

#[test]
fn frames_input_recovery_and_stale_window_disconnect_are_session_local() {
    let (_dir, root) = engine();
    let first_request = request("office.local", ConnectionProtocol::Vnc);
    let first = root.connect_request(first_request.clone()).unwrap();
    let second = first
        .connect_request(request("office.local", ConnectionProtocol::Rdp))
        .unwrap();
    let (a_cmd, mut a_rx) = tokio::sync::mpsc::channel(8);
    let (b_cmd, mut b_rx) = tokio::sync::mpsc::channel(8);
    let a = attempt(&first);
    let b = attempt(&second);
    let a_frames = a.publish(a_cmd, "VNC".into()).unwrap();
    let b_frames = b.publish(b_cmd, "RDP".into()).unwrap();
    let mut a_view = first.take_client_frames().unwrap();
    let mut b_view = second.take_client_frames().unwrap();
    a_frames
        .send(VideoFrame {
            width: 1,
            height: 1,
            data: vec![10; 4],
            pts_us: 0,
        })
        .unwrap();
    b_frames
        .send(VideoFrame {
            width: 1,
            height: 1,
            data: vec![20; 4],
            pts_us: 0,
        })
        .unwrap();
    assert_eq!(root.rt.block_on(a_view.recv()).unwrap().data, vec![10; 4]);
    assert_eq!(root.rt.block_on(b_view.recv()).unwrap().data, vec![20; 4]);
    first.set_frame_geometry(a.generation, 640, 480);
    second.set_frame_geometry(b.generation, 1920, 1080);
    assert!(matches!(
        a_rx.try_recv().unwrap(),
        ControlMsg::FrameGeometry { width: 640, .. }
    ));
    assert!(matches!(
        b_rx.try_recv().unwrap(),
        ControlMsg::FrameGeometry { width: 1920, .. }
    ));
    a.pause_input();
    assert!(!first.client_input_ready(a.generation));
    assert!(second.client_input_ready(b.generation));
    assert!(root.connect_request(first_request.clone()).is_err());

    first.disconnect_client_if(a.generation);
    drop(a_frames);
    assert!(root.rt.block_on(a_view.recv()).is_none());
    assert!(!b_frames.is_closed());
    assert!(
        root.session_active(),
        "updates must still wait for the other viewer"
    );
    let replacement = root.connect_request(first_request).unwrap();
    first.disconnect_client_if(a.generation);
    assert!(replacement.client_task.lock().unwrap().is_some());
    assert!(second.client_input_ready(b.generation));
    assert!(
        !UiEvent::ConnectFailed {
            generation: a.generation,
            error: "old".into()
        }
        .belongs_to_client(replacement.client_generation())
    );
    second.disconnect_client();
    replacement.disconnect_client();
    assert!(!root.session_active());
}

#[test]
fn resolved_addresses_and_native_fingerprints_reject_aliases() {
    let (_dir, root) = engine();
    let first = root
        .connect_request(request("office.local", ConnectionProtocol::Removent))
        .unwrap();
    attempt(&first)
        .claim_addresses(&["192.0.2.10:48688".parse().unwrap()])
        .unwrap();
    attempt(&first).claim_fingerprint([42; 32]).unwrap();
    assert!(
        root.connect_request(request("192.0.2.10", ConnectionProtocol::Removent))
            .is_err()
    );
    let alias = root
        .connect_request(request("alias.local", ConnectionProtocol::Removent))
        .unwrap();
    assert!(
        attempt(&alias)
            .claim_addresses(&["192.0.2.10:50000".parse().unwrap()])
            .is_err()
    );
    assert!(attempt(&alias).claim_fingerprint([42; 32]).is_err());
    alias.disconnect_client();
    first.disconnect_client();
}

#[test]
fn relay_rooms_are_distinct_and_pinned_hosts_share_direct_reservations() {
    use removent_client::connection::{RelayRoute, RelayTransport};
    let (_dir, root) = engine();
    let mut relay = request("office", ConnectionProtocol::Removent);
    relay.address = ConnectionAddress::relay_room("office").unwrap();
    relay.relay = Some(
        RelayRoute::parse(
            "removent://relay.example:443",
            RelayTransport::WebSocket,
            "",
            "",
        )
        .unwrap(),
    );
    let first = root.connect_request(relay.clone()).unwrap();
    assert!(root.connect_request(relay.clone()).is_err());
    relay.address = ConnectionAddress::relay_room("studio").unwrap();
    let second = root.connect_request(relay.clone()).unwrap();
    attempt(&first).claim_fingerprint([42; 32]).unwrap();
    relay.address = ConnectionAddress::relay_room("another-route").unwrap();
    relay.relay.as_mut().unwrap().host_fingerprint = hex::encode([42; 32]);
    assert!(root.connect_request(relay).is_err());
    first.disconnect_client();
    second.disconnect_client();
}

#[test]
fn failed_tasks_release_reservations_for_retry() {
    let (_dir, root) = engine();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut request = request("127.0.0.1", ConnectionProtocol::Vnc);
    request.address.port = listener.local_addr().unwrap().port();
    drop(listener);
    let first = root.connect_request(request.clone()).unwrap();
    root.rt.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !first
                .client_task
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .is_finished()
            {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    });
    assert!(matches!(
        first.client_recovery().state(),
        removent_client::recovery::State::Failed(_)
    ));
    let retry = root.connect_request(request).unwrap();
    retry.disconnect_client();
    assert!(!root.session_active());
}

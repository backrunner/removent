use super::*;
use removent_core::{MemoryClipboard, TextClipboard};
use removent_proto::{KeyKind, KeyModifiers};

fn paste(kind: KeyKind) -> ControlMsg {
    ControlMsg::KeyEvent {
        vk_code: 9,
        modifiers: KeyModifiers::COMMAND,
        kind,
        unicode: None,
    }
}

async fn next_message(
    reader: &mut removent_net::clipboard::ClipboardReader,
    peer: &mut Peer,
) -> ControlMsg {
    let item = tokio::time::timeout(Duration::from_secs(2), reader.next(&mut peer.source))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let ControlItem::Msg(message) = item else {
        panic!("expected message")
    };
    *message
}

#[tokio::test]
async fn paste_sends_existing_unicode_text_and_waits_for_matching_ack() {
    let clip = MemoryClipboard::new();
    clip.write("连接前复制\nUnicode 🚀").unwrap();
    let (session, mut peer) = fixture(Caps::all(), Some(clip.clone())).await;
    let (mut reader, _outbound) = removent_net::clipboard::ClipboardReader::new(peer.conn.clone());
    session.input_tx.send(paste(KeyKind::Down)).await.unwrap();
    session.input_tx.send(paste(KeyKind::Up)).await.unwrap();
    let ControlMsg::ClipboardSync { seq, data, .. } = next_message(&mut reader, &mut peer).await
    else {
        panic!()
    };
    assert_eq!(data, "连接前复制\nUnicode 🚀".as_bytes());
    peer.sink
        .send(ControlMsg::ClipboardAck {
            seq: seq.wrapping_add(1),
        })
        .await
        .unwrap();
    peer.sink
        .send(ControlMsg::Ping { ts_us: 42 })
        .await
        .unwrap();
    assert_eq!(
        next_message(&mut reader, &mut peer).await,
        ControlMsg::Pong { ts_us: 42 }
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(80), reader.next(&mut peer.source))
            .await
            .is_err(),
        "paste must wait for its own ack"
    );
    peer.sink
        .send(ControlMsg::ClipboardAck { seq })
        .await
        .unwrap();
    assert_eq!(
        next_message(&mut reader, &mut peer).await,
        paste(KeyKind::Down)
    );
    assert_eq!(
        next_message(&mut reader, &mut peer).await,
        paste(KeyKind::Up)
    );
    session.close().await.unwrap();
}

#[tokio::test]
async fn immediate_copy_then_paste_sends_latest_text_and_empty_paste_clears_stale_text() {
    for text in ["刚复制的新文本", ""] {
        let clip = MemoryClipboard::new();
        clip.write("old").unwrap();
        let (session, mut peer) = fixture(Caps::all(), Some(clip.clone())).await;
        let (mut reader, _outbound) =
            removent_net::clipboard::ClipboardReader::new(peer.conn.clone());
        clip.write(text).unwrap();
        session.input_tx.send(paste(KeyKind::Down)).await.unwrap();
        // A poll snapshot can arrive first; acknowledge all snapshots until the
        // paste key arrives, and verify every one contains the new content.
        loop {
            match next_message(&mut reader, &mut peer).await {
                ControlMsg::ClipboardSync { seq, data, .. } => {
                    assert_eq!(data, text.as_bytes());
                    peer.sink
                        .send(ControlMsg::ClipboardAck { seq })
                        .await
                        .unwrap();
                }
                message => {
                    assert_eq!(message, paste(KeyKind::Down));
                    break;
                }
            }
        }
        session.close().await.unwrap();
    }
}

#[tokio::test]
async fn missing_paste_ack_closes_session_without_sending_paste() {
    let clip = MemoryClipboard::new();
    clip.write("do not paste old text").unwrap();
    let (session, mut peer) = fixture(Caps::all(), Some(clip)).await;
    let (mut reader, _outbound) = removent_net::clipboard::ClipboardReader::new(peer.conn.clone());
    session.input_tx.send(paste(KeyKind::Down)).await.unwrap();
    session.input_tx.send(paste(KeyKind::Up)).await.unwrap();
    assert!(matches!(
        next_message(&mut reader, &mut peer).await,
        ControlMsg::ClipboardSync { .. }
    ));
    let item = tokio::time::timeout(Duration::from_secs(6), peer.source.next())
        .await
        .unwrap();
    assert!(
        !matches!(item, Some(Ok(ControlItem::Msg(message))) if matches!(*message, ControlMsg::KeyEvent { .. }))
    );
    assert!(session.conn.inner().close_reason().is_some());
}

#[tokio::test]
async fn clipboard_opt_out_preserves_remote_paste_shortcut() {
    let (session, mut peer) = fixture(
        Caps {
            clipboard: false,
            ..Caps::all()
        },
        Some(MemoryClipboard::new()),
    )
    .await;
    session.input_tx.send(paste(KeyKind::Down)).await.unwrap();
    let ControlItem::Msg(message) = peer.source.next().await.unwrap().unwrap() else {
        panic!()
    };
    assert_eq!(*message, paste(KeyKind::Down));
    session.close().await.unwrap();
}

struct ClipboardInput {
    clip: Arc<MemoryClipboard>,
    events: mpsc::UnboundedSender<(ControlMsg, String)>,
}

impl removent_host::InputSink for ClipboardInput {
    fn mouse(
        &self,
        _: u64,
        _: f32,
        _: f32,
        _: u8,
        _: removent_proto::MouseKind,
    ) -> Result<(), String> {
        Ok(())
    }
    fn scroll(&self, _: u64, _: f32, _: f32, _: removent_proto::ScrollPhase) -> Result<(), String> {
        Ok(())
    }
    fn key(
        &self,
        vk_code: u16,
        modifiers: KeyModifiers,
        kind: KeyKind,
        unicode: Option<char>,
    ) -> Result<(), String> {
        let _ = self.events.send((
            ControlMsg::KeyEvent {
                vk_code,
                modifiers,
                kind,
                unicode,
            },
            self.clip.read()?,
        ));
        Ok(())
    }
}

#[tokio::test]
async fn real_host_pump_pastes_new_text_and_shares_clipboard_both_ways_without_echo() {
    let client_clip = MemoryClipboard::new();
    client_clip.write("existing client text").unwrap();
    let host_clip = MemoryClipboard::new();
    host_clip.write("stale host text").unwrap();
    let (session, peer) = fixture(Caps::all(), Some(client_clip.clone())).await;
    let host_state = Arc::new(ClipSyncState::new(host_clip.as_ref()));
    let (events, mut input) = mpsc::unbounded_channel();
    let (tx, rx) = mpsc::channel(8);
    let cancel = tokio_util::sync::CancellationToken::new();
    let _stop = cancel.clone().drop_guard();
    let poller = AbortOnDrop(removent_core::spawn_clipboard_poller(
        host_clip.clone(),
        host_state.clone(),
        tx,
        20,
    ));
    let pump = removent_host::spawn_control_pump(
        peer.source,
        peer.sink,
        removent_host::ControlPumpDeps {
            automation: None,
            conn: peer.conn,
            kf_tx: None,
            controller: None,
            window_ms: 250,
            input: Some(Arc::new(ClipboardInput {
                clip: host_clip.clone(),
                events,
            })),
            local_clip: Some(host_clip.clone()),
            quality_tx: None,
            caps: Caps::all(),
            clip_state: Some(host_state),
            cancel,
            peer_fp: None,
            delivery: None,
        },
        rx,
    );
    let pump = AbortOnDrop(pump);
    session.input_tx.send(paste(KeyKind::Down)).await.unwrap();
    session.input_tx.send(paste(KeyKind::Up)).await.unwrap();
    for kind in [KeyKind::Down, KeyKind::Up] {
        let (event, text) = tokio::time::timeout(Duration::from_secs(2), input.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event, paste(kind));
        assert_eq!(text, "existing client text");
    }
    // Exercise the actual host and client control pumps and both pollers.
    for (from, to, text) in [
        (&host_clip, &client_clip, "remote copy 中文"),
        (&client_clip, &host_clip, "local copy 🚀"),
    ] {
        from.write(text).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while to.read().unwrap() != text {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let counts = (from.change_count().unwrap(), to.change_count().unwrap());
        tokio::time::sleep(Duration::from_millis(550)).await;
        assert_eq!(
            (from.change_count().unwrap(), to.change_count().unwrap()),
            counts,
            "clipboard writes must not echo"
        );
    }
    session.close().await.unwrap();
    drop((pump, poller));
}

#[test]
fn paste_detection_preserves_other_shortcuts() {
    assert!(super::super::control::is_paste(&paste(KeyKind::Down)));
    assert!(!super::super::control::is_paste(&paste(KeyKind::Up)));
    let mut event = paste(KeyKind::Down);
    if let ControlMsg::KeyEvent { modifiers, .. } = &mut event {
        *modifiers |= KeyModifiers::OPTION | KeyModifiers::SHIFT;
    }
    assert!(super::super::control::is_paste(&event));
    if let ControlMsg::KeyEvent { modifiers, .. } = &mut event {
        *modifiers |= KeyModifiers::CONTROL;
    }
    assert!(!super::super::control::is_paste(&event));
}

#[tokio::test]
async fn reliable_snapshot_survives_background_replacement_and_rejects_delayed_old_snapshot() {
    let clip = MemoryClipboard::new();
    let (session, mut peer) = fixture(Caps::all(), Some(clip.clone())).await;
    let (_reader, sender) = removent_net::clipboard::ClipboardReader::new(peer.conn.clone());
    let snapshot = |seq, text: &str| ControlMsg::ClipboardSync {
        seq,
        format: removent_proto::ClipFormat::TextUtf8,
        data: text.as_bytes().to_vec(),
    };
    removent_net::clipboard::enqueue(&sender, snapshot(0, "stale")).unwrap();
    sender
        .send_reliable(snapshot(1, "forced paste"))
        .await
        .unwrap();
    // This stale poll result reaches the sender after the reliable snapshot.
    removent_net::clipboard::enqueue(&sender, snapshot(0, "delayed stale")).unwrap();
    let ack = tokio::time::timeout(Duration::from_secs(2), peer.source.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(ack, ControlItem::Msg(message) if *message == ControlMsg::ClipboardAck { seq: 1 })
    );
    assert_eq!(clip.read().unwrap(), "forced paste");
    assert!(
        tokio::time::timeout(Duration::from_millis(300), peer.source.next())
            .await
            .is_err()
    );
    removent_net::clipboard::enqueue(&sender, snapshot(2, "next copy")).unwrap();
    let ack = tokio::time::timeout(Duration::from_secs(2), peer.source.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(ack, ControlItem::Msg(message) if *message == ControlMsg::ClipboardAck { seq: 2 })
    );
    assert_eq!(clip.read().unwrap(), "next copy");
    session.close().await.unwrap();
}

struct UnavailableClipboard;
impl TextClipboard for UnavailableClipboard {
    fn change_count(&self) -> Result<u64, String> {
        Ok(1)
    }
    fn read(&self) -> Result<String, String> {
        Err("text unavailable".into())
    }
    fn write(&self, _: &str) -> Result<(), String> {
        Err("clipboard write failed".into())
    }
}

#[tokio::test]
async fn unavailable_or_oversized_local_text_does_not_paste_remote_stale_text() {
    let large = MemoryClipboard::new();
    large
        .write(&"x".repeat(removent_core::clip::MAX_CLIPBOARD_BYTES + 1))
        .unwrap();
    for clip in [
        Arc::new(UnavailableClipboard) as Arc<dyn TextClipboard>,
        large,
    ] {
        let (session, mut peer) = fixture(Caps::all(), Some(clip)).await;
        session.input_tx.send(paste(KeyKind::Down)).await.unwrap();
        let next_key = ControlMsg::KeyEvent {
            vk_code: 7,
            modifiers: KeyModifiers::empty(),
            kind: KeyKind::Down,
            unicode: Some('x'),
        };
        session.input_tx.send(next_key.clone()).await.unwrap();
        let item = tokio::time::timeout(Duration::from_secs(2), peer.source.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(item, ControlItem::Msg(message) if *message == next_key));
        session.close().await.unwrap();
    }
}

#[tokio::test]
async fn host_write_failure_ends_session_without_acknowledging_or_pasting() {
    let clip = MemoryClipboard::new();
    clip.write("new text").unwrap();
    let (session, peer) = fixture(Caps::all(), Some(clip)).await;
    let recorder = Arc::new(removent_host::RecorderInputSink::default());
    let (_tx, rx) = mpsc::channel(4);
    let cancel = tokio_util::sync::CancellationToken::new();
    let _stop = cancel.clone().drop_guard();
    let pump = removent_host::spawn_control_pump(
        peer.source,
        peer.sink,
        removent_host::ControlPumpDeps {
            automation: None,
            conn: peer.conn,
            kf_tx: None,
            controller: None,
            window_ms: 250,
            input: Some(recorder.clone()),
            local_clip: Some(Arc::new(UnavailableClipboard)),
            quality_tx: None,
            caps: Caps::all(),
            clip_state: None,
            cancel: cancel.clone(),
            peer_fp: None,
            delivery: None,
        },
        rx,
    );
    session.input_tx.send(paste(KeyKind::Down)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), pump)
        .await
        .unwrap()
        .unwrap();
    assert!(cancel.is_cancelled());
    assert!(
        recorder.events.lock().unwrap().is_empty(),
        "no paste key may reach input injection"
    );
    session.close().await.unwrap();
}

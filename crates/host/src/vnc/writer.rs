use super::framebuffer::send_frame_message;
use super::input::{key_for_keysym, pointer_button};
use super::{ClientMessage, DisplayTarget, Frame, default_pixel_format};
use crate::input_sink::{BUTTON_LEFT, BUTTON_MIDDLE, BUTTON_RIGHT, InputReleaseTracker, InputSink};
use removent_proto::{KeyKind, KeyModifiers, MouseKind, ScrollPhase};
use std::io;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct InputCleanup {
    tracker: InputReleaseTracker,
    input: Option<Arc<dyn InputSink>>,
}

impl Drop for InputCleanup {
    fn drop(&mut self) {
        if let Some(input) = &self.input {
            self.tracker.release_all(input.as_ref());
        }
    }
}

pub(super) async fn write_frames(
    mut stream: impl tokio::io::AsyncWrite + Unpin + Send + 'static,
    mut frame_rx: removent_core::latest::Receiver<Frame>,
    mut msg_rx: mpsc::Receiver<ClientMessage>,
    target: DisplayTarget,
    input: Option<Arc<dyn InputSink>>,
    shutdown: CancellationToken,
) -> io::Result<()> {
    let mut cleanup = InputCleanup {
        tracker: Default::default(),
        input: input.clone(),
    };
    // A blocked TCP write must not hold input releases behind a large frame.
    // At most one packet is in flight; capture still uses a latest-frame slot.
    let (packet_tx, mut packet_rx) = mpsc::channel::<Vec<u8>>(1);
    let (sent_tx, mut sent_rx) = mpsc::channel(1);
    let mut writers = tokio::task::JoinSet::new();
    writers.spawn(async move {
        while let Some(packet) = packet_rx.recv().await {
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                stream.write_all(&packet),
            )
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "VNC frame write timed out"))??;
            if sent_tx.send(()).await.is_err() {
                break;
            }
        }
        Ok::<(), io::Error>(())
    });
    let work = async {
        let mut latest: Option<Frame> = None;
        let mut request: Option<(bool, u16, u16, u16, u16)> = None;
        let mut button_mask = 0u8;
        let mut modifiers = KeyModifiers::empty();
        let mut pixel_format = default_pixel_format();
        let mut writing = false;
        let mut fresh_frame = false;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                result = writers.join_next() => {
                    return result.ok_or_else(|| io::Error::other("VNC writer task disappeared"))?
                        .map_err(|e| io::Error::other(e.to_string()))?;
                }
                sent = sent_rx.recv(), if writing => {
                    if sent.is_none() { continue; }
                    writing = false;
                }
                frame = frame_rx.recv() => {
                    let Some(frame) = frame else { break };
                    latest = Some(frame);
                    fresh_frame = true;
                }
                msg = msg_rx.recv() => {
                    let Some(msg) = msg else { break };
                    match msg {
                    ClientMessage::SetPixelFormat(format) => {
                        if format.supported() {
                            pixel_format = format;
                        } else {
                            return Err(io::Error::new(io::ErrorKind::InvalidData, "unsupported VNC pixel format"));
                        }
                    }
                    ClientMessage::FramebufferRequest { incremental, x, y, width, height } => {
                        request = Some((incremental, x, y, width, height));
                    }
                    ClientMessage::Pointer { mask, x, y } => {
                        let ours = ((mask & 1 != 0) as u8 * BUTTON_LEFT)
                            | ((mask & 2 != 0) as u8 * BUTTON_MIDDLE)
                            | ((mask & 4 != 0) as u8 * BUTTON_RIGHT);
                        if let Some(sink) = input.as_ref() {
                            for kind in [
                                pointer_button(ours, button_mask, BUTTON_LEFT),
                                pointer_button(ours, button_mask, BUTTON_RIGHT),
                                pointer_button(ours, button_mask, BUTTON_MIDDLE),
                            ]
                            .into_iter()
                            .flatten()
                            {
                                let _ = sink.mouse(target.id, x as f32, y as f32, ours, kind);
                                cleanup.tracker.note_mouse(target.id, x as f32, y as f32, kind);
                            }
                            if mask & 8 != 0 && button_mask & 8 == 0 { let _ = sink.scroll(target.id, 0.0, 10.0, ScrollPhase::Began); let _ = sink.scroll(target.id, 0.0, 10.0, ScrollPhase::Ended); }
                            if mask & 16 != 0 && button_mask & 16 == 0 { let _ = sink.scroll(target.id, 0.0, -10.0, ScrollPhase::Began); let _ = sink.scroll(target.id, 0.0, -10.0, ScrollPhase::Ended); }
                            if ours == button_mask { let _ = sink.mouse(target.id, x as f32, y as f32, ours, MouseKind::Moved); cleanup.tracker.note_mouse(target.id, x as f32, y as f32, MouseKind::Moved); }
                        }
                        button_mask = ours;
                    }
                    ClientMessage::Key { down, keysym } => {
                        if let Some((vk, modifier)) = key_for_keysym(keysym) {
                            let modifier_key =
                                matches!(keysym, 0xffe1..=0xffe5 | 0xffe9..=0xffec);
                            if let Some(bit) = modifier.filter(|_| modifier_key) {
                                if down { modifiers.insert(bit); } else { modifiers.remove(bit); }
                                if let Some(sink) = input.as_ref() { let _ = sink.key(vk, modifiers, KeyKind::FlagsChanged, None); cleanup.tracker.note_key(vk, modifiers, KeyKind::FlagsChanged); }
                            } else if let Some(sink) = input.as_ref() {
                                let kind = if down { KeyKind::Down } else { KeyKind::Up };
                                let unicode = (keysym <= 0xff && (keysym as u8).is_ascii_graphic()).then_some(keysym as u8 as char);
                                let effective_modifiers = modifiers | modifier.unwrap_or_default();
                                let _ = sink.key(vk, effective_modifiers, kind, unicode);
                                cleanup.tracker.note_key(vk, effective_modifiers, kind);
                            }
                        }
                    }
                        ClientMessage::Other => {}
                    }
                },
            }
            if !writing
                && let Some((incremental, x, y, w, h)) = request
                && (!incremental || fresh_frame)
                && let Some(frame) = latest.as_ref()
            {
                let mut packet = Vec::with_capacity(16 + frame.data.len());
                send_frame_message(&mut packet, frame, x, y, w, h, pixel_format);
                packet_tx
                    .try_send(packet)
                    .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "VNC writer closed"))?;
                writing = true;
                fresh_frame = false;
                request = None;
            }
        }
        Ok(())
    };
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => Ok(()),
        result = work => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_sink::{RecordedInput, RecorderInputSink};
    use std::time::Duration;

    #[tokio::test]
    async fn stalled_frame_write_does_not_block_key_release() {
        use tokio::io::AsyncReadExt;
        let (stream, mut peer) = tokio::io::duplex(32);
        let (frames, frame_rx) = removent_core::latest::channel();
        let (messages, msg_rx) = mpsc::channel(4);
        let input = Arc::new(RecorderInputSink::default());
        let stop = CancellationToken::new();
        let target = DisplayTarget {
            id: 1,
            capture_width: 64,
            capture_height: 64,
            width: 64,
            height: 64,
        };
        let task = tokio::spawn(write_frames(
            stream,
            frame_rx,
            msg_rx,
            target,
            Some(input.clone()),
            stop.clone(),
        ));
        assert!(
            frames
                .send(Frame {
                    data: vec![0; 64 * 64 * 4],
                    width: 64,
                    height: 64,
                })
                .is_ok()
        );
        messages
            .send(ClientMessage::FramebufferRequest {
                incremental: false,
                x: 0,
                y: 0,
                width: 64,
                height: 64,
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), peer.read_exact(&mut [0; 32]))
            .await
            .unwrap()
            .unwrap();
        // Leave the remaining frame blocked in the 32-byte transport buffer.
        for down in [true, false] {
            messages
                .send(ClientMessage::Key {
                    down,
                    keysym: 'a' as u32,
                })
                .await
                .unwrap();
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if input.events.lock().unwrap().iter().any(|event| {
                    matches!(
                        event,
                        RecordedInput::Key {
                            kind: KeyKind::Up,
                            ..
                        }
                    )
                }) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("input release waited behind blocked video");
        stop.cancel();
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn stalled_or_failed_frame_write_always_releases_held_keys() {
        for mode in 0..3 {
            let (stream, peer) = tokio::io::duplex(32);
            let (frames, frame_rx) = removent_core::latest::channel();
            let (messages, msg_rx) = mpsc::channel(4);
            let input = Arc::new(RecorderInputSink::default());
            let stop = CancellationToken::new();
            let target = DisplayTarget {
                id: 1,
                capture_width: 64,
                capture_height: 64,
                width: 64,
                height: 64,
            };
            let mut task = tokio::spawn(write_frames(
                stream,
                frame_rx,
                msg_rx,
                target,
                Some(input.clone()),
                stop.clone(),
            ));
            messages
                .send(ClientMessage::Key {
                    down: true,
                    keysym: 'a' as u32,
                })
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(1), async {
                while input.events.lock().unwrap().is_empty() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            messages
                .send(ClientMessage::FramebufferRequest {
                    incremental: false,
                    x: 0,
                    y: 0,
                    width: 64,
                    height: 64,
                })
                .await
                .unwrap();
            assert!(
                frames
                    .send(Frame {
                        data: vec![0; 64 * 64 * 4],
                        width: 64,
                        height: 64
                    })
                    .is_ok()
            );
            tokio::task::yield_now().await;
            assert!(!task.is_finished());
            match mode {
                0 => stop.cancel(),
                1 => task.abort(),
                _ => drop(peer),
            }
            let result = tokio::time::timeout(Duration::from_secs(1), &mut task)
                .await
                .unwrap();
            match mode {
                0 => assert!(result.unwrap().is_ok()),
                1 => assert!(result.is_err()),
                _ => assert!(result.unwrap().is_err()),
            }
            let events = input.events.lock().unwrap();
            assert_eq!(events.len(), 2);
            assert!(matches!(
                events[1],
                RecordedInput::Key {
                    kind: KeyKind::Up,
                    ..
                }
            ));
        }
    }
}

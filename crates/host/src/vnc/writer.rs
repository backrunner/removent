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

pub(super) async fn write_frames(
    mut stream: tokio::net::tcp::OwnedWriteHalf,
    mut frame_rx: mpsc::Receiver<Frame>,
    mut msg_rx: mpsc::Receiver<ClientMessage>,
    target: DisplayTarget,
    input: Option<Arc<dyn InputSink>>,
    shutdown: CancellationToken,
) -> io::Result<()> {
    let mut latest: Option<Frame> = None;
    let mut request: Option<(bool, u16, u16, u16, u16)> = None;
    let mut button_mask = 0u8;
    let mut modifiers = KeyModifiers::empty();
    let mut tracker = InputReleaseTracker::default();
    let mut pixel_format = default_pixel_format();
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            frame = frame_rx.recv() => {
                let Some(frame) = frame else { break };
                latest = Some(frame);
                if let Some((_, x, y, w, h)) = request.take() {
                    let frame = latest.as_ref().unwrap();
                    let mut packet = Vec::with_capacity(16 + frame.data.len());
                    send_frame_message(&mut packet, frame, x, y, w, h, pixel_format);
                    stream.write_all(&packet).await?;
                }
            }
            msg = msg_rx.recv() => {
                let Some(msg) = msg else { break };
                match msg {
                ClientMessage::SetPixelFormat(format) => {
                    if format.supported() {
                        pixel_format = format;
                    } else {
                        tracing::debug!(
                            bits_per_pixel = format.bits_per_pixel,
                            depth = format.depth,
                            "ignoring unsupported VNC pixel format"
                        );
                    }
                }
                ClientMessage::FramebufferRequest { incremental, x, y, width, height } => {
                    request = Some((incremental, x, y, width, height));
                    if !incremental && let Some(frame) = latest.as_ref() {
                        let mut packet = Vec::with_capacity(16 + frame.data.len());
                        send_frame_message(&mut packet, frame, x, y, width, height, pixel_format);
                        stream.write_all(&packet).await?;
                        request = None;
                    }
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
                            tracker.note_mouse(target.id, x as f32, y as f32, kind);
                        }
                        if mask & 8 != 0 && button_mask & 8 == 0 { let _ = sink.scroll(target.id, 0.0, 10.0, ScrollPhase::Began); let _ = sink.scroll(target.id, 0.0, 10.0, ScrollPhase::Ended); }
                        if mask & 16 != 0 && button_mask & 16 == 0 { let _ = sink.scroll(target.id, 0.0, -10.0, ScrollPhase::Began); let _ = sink.scroll(target.id, 0.0, -10.0, ScrollPhase::Ended); }
                        if ours == button_mask { let _ = sink.mouse(target.id, x as f32, y as f32, ours, MouseKind::Moved); tracker.note_mouse(target.id, x as f32, y as f32, MouseKind::Moved); }
                    }
                    button_mask = ours;
                }
                ClientMessage::Key { down, keysym } => {
                    if let Some((vk, modifier)) = key_for_keysym(keysym) {
                        let modifier_key =
                            matches!(keysym, 0xffe1..=0xffe5 | 0xffe9..=0xffec);
                        if let Some(bit) = modifier.filter(|_| modifier_key) {
                            if down { modifiers.insert(bit); } else { modifiers.remove(bit); }
                            if let Some(sink) = input.as_ref() { let _ = sink.key(vk, modifiers, KeyKind::FlagsChanged, None); tracker.note_key(vk, modifiers, KeyKind::FlagsChanged); }
                        } else if let Some(sink) = input.as_ref() {
                            let kind = if down { KeyKind::Down } else { KeyKind::Up };
                            let unicode = (keysym <= 0xff && (keysym as u8).is_ascii_graphic()).then_some(keysym as u8 as char);
                            let effective_modifiers = modifiers | modifier.unwrap_or_default();
                            let _ = sink.key(vk, effective_modifiers, kind, unicode);
                            tracker.note_key(vk, effective_modifiers, kind);
                        }
                    }
                }
                    ClientMessage::Other => {}
                }
            },
        }
    }
    if let Some(sink) = input.as_ref() {
        tracker.release_all(sink.as_ref());
    }
    Ok(())
}

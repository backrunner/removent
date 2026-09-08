use super::framebuffer::write_framebuffer_request;
use super::{FrameSignal, FrameSize};
use removent_proto::{ControlMsg, KeyKind, KeyModifiers};
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::{RwLock, mpsc};

pub(super) async fn write_commands(
    mut stream: tokio::net::tcp::OwnedWriteHalf,
    mut cmd_rx: super::queue::InputReceiver,
    mut signal_rx: mpsc::Receiver<FrameSignal>,
    dimensions: Arc<RwLock<FrameSize>>,
    apple_ard: bool,
) {
    let mut input = InputState::default();
    let mut burst = 0;
    loop {
        tokio::select! {
            biased;
            cmd = cmd_rx.recv(), if burst < 32 => {
                let Some(cmd) = cmd else { break };
                let is_input = matches!(cmd.message, ControlMsg::MouseEvent { .. } | ControlMsg::KeyEvent { .. } | ControlMsg::ScrollEvent { .. });
                if let Err(error) = write_command(&mut stream, cmd.message, &mut input, apple_ard).await {
                    tracing::warn!(%error, "VNC stream write failed");
                    break;
                }
                cmd_rx.record_sent(cmd.queued_at, is_input);
                burst += 1;
            }
            signal = signal_rx.recv() => match signal {
                Some(FrameSignal::Updated) => {
                    burst = 0;
                    let size = *dimensions.read().await;
                    if size.width > 0 && size.height > 0
                        && let Err(error) = write_framebuffer_request(&mut stream, true, size.width, size.height).await {
                        tracing::warn!(%error, "VNC framebuffer request failed");
                        break;
                    }
                }
                Some(FrameSignal::Closed) | None => break,
            },
            // Give frame requests and the runtime a turn during sustained input,
            // without waiting for a frame to arrive before sending more keys.
            _ = tokio::task::yield_now(), if burst >= 32 => { burst = 0; }
        }
    }
}

#[derive(Default)]
struct InputState {
    last_x: u16,
    last_y: u16,
    buttons: u8,
    pressed_keys: HashMap<u16, u32>,
}

async fn write_command(
    stream: &mut (impl tokio::io::AsyncWrite + Unpin),
    cmd: ControlMsg,
    input: &mut InputState,
    apple_ard: bool,
) -> io::Result<()> {
    match cmd {
        ControlMsg::MouseEvent {
            x_px,
            y_px,
            buttons,
            ..
        } => {
            // ControlMsg uses Removent's left/right/middle bit order
            // (1/2/4). Standard RFB uses left/middle/right (1/2/4), while
            // Apple's ARD messages use the Removent/macOS order. Keep the
            // native order for ARD and swap the latter two bits for RFB.
            let mask = rfb_button_mask(buttons, apple_ard);
            input.buttons = mask;
            input.last_x = x_px.clamp(0.0, u16::MAX as f32) as u16;
            input.last_y = y_px.clamp(0.0, u16::MAX as f32) as u16;
            let mut msg = [0u8; 6];
            msg[0] = 5;
            msg[1] = mask;
            msg[2..4].copy_from_slice(&input.last_x.to_be_bytes());
            msg[4..6].copy_from_slice(&input.last_y.to_be_bytes());
            stream.write_all(&msg).await
        }
        ControlMsg::ScrollEvent { dx_mm, dy_mm, .. } => {
            for (delta, negative, positive) in [(dy_mm, 8, 16), (dx_mm, 32, 64)] {
                if !delta.is_finite() || delta == 0.0 {
                    continue;
                }
                let button = if delta < 0.0 { negative } else { positive };
                let mut down = [0u8; 6];
                down[0] = 5;
                down[1] = input.buttons | button;
                down[2..4].copy_from_slice(&input.last_x.to_be_bytes());
                down[4..6].copy_from_slice(&input.last_y.to_be_bytes());
                let mut up = down;
                up[1] = input.buttons;
                stream.write_all(&down).await?;
                stream.write_all(&up).await?;
            }
            Ok(())
        }
        ControlMsg::KeyEvent {
            vk_code,
            modifiers,
            kind,
            unicode,
        } => {
            let mut msg = [0u8; 8];
            msg[0] = 4;
            let down = match kind {
                KeyKind::Down => true,
                KeyKind::Up => false,
                KeyKind::FlagsChanged => {
                    modifier_for_vk(vk_code).is_some_and(|bit| modifiers.contains(bit))
                }
            };
            // A release must use the exact keysym sent on key-down, even if
            // modifiers changed or focus loss supplied no character.
            let keysym = if down {
                *input
                    .pressed_keys
                    .entry(vk_code)
                    .or_insert_with(|| keysym_for_key(vk_code, modifiers, unicode))
            } else {
                input
                    .pressed_keys
                    .remove(&vk_code)
                    .unwrap_or_else(|| keysym_for_key(vk_code, modifiers, unicode))
            };
            if keysym == 0 {
                input.pressed_keys.remove(&vk_code);
                return Ok(());
            }
            msg[1] = u8::from(down);
            msg[4..8].copy_from_slice(&keysym.to_be_bytes());
            // ARD type-30 encrypts only the credential exchange. Subsequent
            // input uses ordinary RFB key events (as implemented by
            // noVNC/gtk-vnc); do not send the unrelated private 0x10 event.
            stream.write_all(&msg).await?;
            if down && vk_code == u16::MAX {
                // Unicode-only input identifies text, not a held physical key.
                input.pressed_keys.remove(&vk_code);
                msg[1] = 0;
                stream.write_all(&msg).await?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

pub(super) fn rfb_button_mask(buttons: u8, apple_ard: bool) -> u8 {
    if apple_ard {
        // ARD uses left/right/middle (1/2/4), matching ControlMsg.
        buttons & 0x07
    } else {
        // Standard RFB uses left/middle/right (1/2/4).
        (buttons & 1) | ((buttons & 2) << 1) | ((buttons & 4) >> 1)
    }
}

pub(super) fn keysym_for_key(vk: u16, modifiers: KeyModifiers, unicode: Option<char>) -> u32 {
    // Named keys keep their X11 keysyms even when key_char contains a
    // control character or a macOS private-use function-key character.
    match vk {
        0x38 | 0x3c => 0xffe1,
        0x3b | 0x3e => 0xffe3,
        0x3a | 0x3d => 0xffe9,
        0x37 | 0x36 => 0xffeb,
        0x39 => 0xffe5,
        0x33 => 0xff08,
        0x30 => 0xff09,
        0x24 => 0xff0d,
        0x35 => 0xff1b,
        0x75 => 0xffff,
        0x7b => 0xff51,
        0x7e => 0xff52,
        0x7c => 0xff53,
        0x7d => 0xff54,
        0x73 => 0xff50,
        0x77 => 0xff57,
        0x74 => 0xff55,
        0x79 => 0xff56,
        0x7a => 0xffbe,
        0x78 => 0xffbf,
        0x63 => 0xffc0,
        0x76 => 0xffc1,
        0x60 => 0xffc2,
        0x61 => 0xffc3,
        0x62 => 0xffc4,
        0x64 => 0xffc5,
        0x65 => 0xffc6,
        0x6d => 0xffc7,
        0x67 => 0xffc8,
        0x6f => 0xffc9,
        _ => text_keysym(vk, modifiers, unicode),
    }
}

fn text_keysym(vk: u16, modifiers: KeyModifiers, unicode: Option<char>) -> u32 {
    if let Some(c) = unicode.filter(|c| !c.is_control()) {
        let code = c as u32;
        return if code <= 0xff {
            code
        } else {
            0x0100_0000 | code
        };
    }
    match vk {
        0x00 => letter_keysym(b'a', modifiers),
        0x0b => letter_keysym(b'b', modifiers),
        0x08 => letter_keysym(b'c', modifiers),
        0x02 => letter_keysym(b'd', modifiers),
        0x0e => letter_keysym(b'e', modifiers),
        0x03 => letter_keysym(b'f', modifiers),
        0x05 => letter_keysym(b'g', modifiers),
        0x04 => letter_keysym(b'h', modifiers),
        0x22 => letter_keysym(b'i', modifiers),
        0x26 => letter_keysym(b'j', modifiers),
        0x28 => letter_keysym(b'k', modifiers),
        0x25 => letter_keysym(b'l', modifiers),
        0x2e => letter_keysym(b'm', modifiers),
        0x2d => letter_keysym(b'n', modifiers),
        0x1f => letter_keysym(b'o', modifiers),
        0x23 => letter_keysym(b'p', modifiers),
        0x0c => letter_keysym(b'q', modifiers),
        0x0f => letter_keysym(b'r', modifiers),
        0x01 => letter_keysym(b's', modifiers),
        0x11 => letter_keysym(b't', modifiers),
        0x20 => letter_keysym(b'u', modifiers),
        0x09 => letter_keysym(b'v', modifiers),
        0x0d => letter_keysym(b'w', modifiers),
        0x07 => letter_keysym(b'x', modifiers),
        0x10 => letter_keysym(b'y', modifiers),
        0x06 => letter_keysym(b'z', modifiers),
        0x12 => b'1' as u32,
        0x13 => b'2' as u32,
        0x14 => b'3' as u32,
        0x15 => b'4' as u32,
        0x17 => b'5' as u32,
        0x16 => b'6' as u32,
        0x1a => b'7' as u32,
        0x1c => b'8' as u32,
        0x19 => b'9' as u32,
        0x1d => b'0' as u32,
        0x1b => b'-' as u32,
        0x18 => b'=' as u32,
        0x21 => b'[' as u32,
        0x1e => b']' as u32,
        0x2a => b'\\' as u32,
        0x29 => b';' as u32,
        0x27 => b'\'' as u32,
        0x2b => b',' as u32,
        0x2f => b'.' as u32,
        0x2c => b'/' as u32,
        0x32 => b'`' as u32,
        0x31 => b' ' as u32,
        _ => 0,
    }
}

fn letter_keysym(base: u8, modifiers: KeyModifiers) -> u32 {
    if modifiers.contains(KeyModifiers::SHIFT) ^ modifiers.contains(KeyModifiers::CAPS_LOCK) {
        (base as char).to_ascii_uppercase() as u32
    } else {
        base as u32
    }
}

fn modifier_for_vk(vk: u16) -> Option<KeyModifiers> {
    match vk {
        0x38 | 0x3c => Some(KeyModifiers::SHIFT),
        0x3b | 0x3e => Some(KeyModifiers::CONTROL),
        0x3a | 0x3d => Some(KeyModifiers::OPTION),
        0x37 | 0x36 => Some(KeyModifiers::COMMAND),
        0x39 => Some(KeyModifiers::CAPS_LOCK),
        0x3f => Some(KeyModifiers::FUNCTION),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[test]
    fn unicode_and_named_keys_use_x11_encoding() {
        let mods = KeyModifiers::empty();
        assert_eq!(keysym_for_key(u16::MAX, mods, Some('中')), 0x0100_4e2d);
        assert_eq!(keysym_for_key(u16::MAX, mods, Some('😀')), 0x0101_f600);
        assert_eq!(keysym_for_key(0x0e, mods, Some('é')), 0xe9);
        assert_eq!(keysym_for_key(0x24, mods, Some('\r')), 0xff0d);
        assert_eq!(keysym_for_key(0x7b, mods, Some('\u{f702}')), 0xff51);
    }

    #[tokio::test]
    async fn release_and_repeat_keep_original_keysym_after_modifiers_change() {
        for (vk, character, expected) in [(0x00, 'A', 0x41u32), (0x0e, '中', 0x0100_4e2d)] {
            let mut input = InputState::default();
            let mut bytes = Vec::new();
            for (kind, modifiers, unicode) in [
                (KeyKind::Down, KeyModifiers::SHIFT, Some(character)),
                (KeyKind::Down, KeyModifiers::empty(), Some('a')),
                (KeyKind::Up, KeyModifiers::empty(), None),
            ] {
                write_command(
                    &mut bytes,
                    ControlMsg::KeyEvent {
                        vk_code: vk,
                        modifiers,
                        kind,
                        unicode,
                    },
                    &mut input,
                    false,
                )
                .await
                .unwrap();
            }
            assert_eq!(bytes.len(), 24);
            for (packet, down) in bytes.as_chunks::<8>().0.iter().zip([1, 1, 0]) {
                assert_eq!(packet[1], down);
                assert_eq!(&packet[4..], &expected.to_be_bytes());
            }
            assert!(input.pressed_keys.is_empty());
        }
    }

    #[tokio::test]
    async fn unicode_only_text_completes_each_character_without_keyup() {
        let mut input = InputState::default();
        let mut bytes = Vec::new();
        for character in ['中', '文'] {
            write_command(
                &mut bytes,
                ControlMsg::KeyEvent {
                    vk_code: u16::MAX,
                    modifiers: KeyModifiers::empty(),
                    kind: KeyKind::Down,
                    unicode: Some(character),
                },
                &mut input,
                false,
            )
            .await
            .unwrap();
        }
        assert_eq!(bytes.len(), 32);
        for (pair, expected) in bytes
            .as_chunks::<16>()
            .0
            .iter()
            .zip([0x0100_4e2du32, 0x0100_6587])
        {
            assert_eq!(pair[1], 1);
            assert_eq!(pair[9], 0);
            assert_eq!(&pair[4..8], &expected.to_be_bytes());
            assert_eq!(&pair[12..16], &expected.to_be_bytes());
        }
        assert!(input.pressed_keys.is_empty());
    }

    #[tokio::test]
    async fn scrolling_preserves_held_buttons_on_both_axes() {
        for apple_ard in [false, true] {
            let mut input = InputState::default();
            let mut bytes = Vec::new();
            write_command(
                &mut bytes,
                ControlMsg::MouseEvent {
                    display_id: 0,
                    x_px: 10.,
                    y_px: 20.,
                    buttons: 3,
                    kind: removent_proto::MouseKind::RightDown,
                },
                &mut input,
                apple_ard,
            )
            .await
            .unwrap();
            bytes.clear();
            write_command(
                &mut bytes,
                ControlMsg::ScrollEvent {
                    display_id: 0,
                    dx_mm: 3.,
                    dy_mm: -3.,
                    phase: removent_proto::ScrollPhase::Changed,
                },
                &mut input,
                apple_ard,
            )
            .await
            .unwrap();
            let held = if apple_ard { 3 } else { 5 };
            assert_eq!(bytes.len(), 24);
            for (packet, mask) in
                bytes
                    .as_chunks::<6>()
                    .0
                    .iter()
                    .zip([held | 8, held, held | 64, held])
            {
                assert_eq!(*packet, [5, mask, 0, 10, 0, 20]);
            }
        }
    }

    #[tokio::test]
    async fn scroll_direction_is_encoded_as_rfb_button_four_or_five() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (mut server, _) = listener.accept().await.unwrap();
        let (_, mut writer) = client.into_split();
        let mut input = InputState {
            last_x: 10,
            last_y: 20,
            ..Default::default()
        };
        for (delta, button) in [(-3., 8), (3., 16)] {
            write_command(
                &mut writer,
                ControlMsg::ScrollEvent {
                    display_id: 0,
                    dx_mm: 0.,
                    dy_mm: delta,
                    phase: removent_proto::ScrollPhase::Changed,
                },
                &mut input,
                false,
            )
            .await
            .unwrap();
            let mut bytes = [0; 12];
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                server.read_exact(&mut bytes),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(bytes, [5, button, 0, 10, 0, 20, 5, 0, 0, 10, 0, 20]);
        }
    }
}

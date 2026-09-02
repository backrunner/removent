use super::framebuffer::write_framebuffer_request;
use super::{FrameSignal, FrameSize};
use removent_proto::{ControlMsg, KeyKind, KeyModifiers};
use std::io;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::{RwLock, mpsc};

pub(super) async fn write_commands(
    mut stream: tokio::net::tcp::OwnedWriteHalf,
    mut cmd_rx: mpsc::Receiver<ControlMsg>,
    mut signal_rx: mpsc::Receiver<FrameSignal>,
    dimensions: Arc<RwLock<FrameSize>>,
    apple_ard: bool,
) {
    let mut last_x = 0u16;
    let mut last_y = 0u16;
    loop {
        tokio::select! {
            signal = signal_rx.recv() => match signal {
                Some(FrameSignal::Updated) => {
                    let size = *dimensions.read().await;
                    if size.width > 0
                        && size.height > 0
                        && write_framebuffer_request(&mut stream, true, size.width, size.height)
                            .await
                            .is_err()
                    {
                        break;
                    }
                }
                Some(FrameSignal::Closed) | None => break,
            },
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { break };
                if write_command(&mut stream, cmd, &mut last_x, &mut last_y, apple_ard).await.is_err() { break; }
            }
        }
    }
}

async fn write_command(
    stream: &mut tokio::net::tcp::OwnedWriteHalf,
    cmd: ControlMsg,
    last_x: &mut u16,
    last_y: &mut u16,
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
            *last_x = x_px.clamp(0.0, u16::MAX as f32) as u16;
            *last_y = y_px.clamp(0.0, u16::MAX as f32) as u16;
            let mut msg = [0u8; 6];
            msg[0] = 5;
            msg[1] = mask;
            msg[2..4].copy_from_slice(&last_x.to_be_bytes());
            msg[4..6].copy_from_slice(&last_y.to_be_bytes());
            stream.write_all(&msg).await
        }
        ControlMsg::ScrollEvent {
            dx_mm: _, dy_mm, ..
        } => {
            if dy_mm == 0.0 {
                return Ok(());
            }
            let button = if dy_mm > 0.0 { 8 } else { 16 };
            let mut down = [0u8; 6];
            down[0] = 5;
            down[1] = button;
            down[2..4].copy_from_slice(&last_x.to_be_bytes());
            down[4..6].copy_from_slice(&last_y.to_be_bytes());
            let mut up = down;
            up[1] = 0;
            stream.write_all(&down).await?;
            stream.write_all(&up).await
        }
        ControlMsg::KeyEvent {
            vk_code,
            modifiers,
            kind,
            unicode,
        } => {
            let keysym = keysym_for_key(vk_code, modifiers, unicode);
            let mut msg = [0u8; 8];
            msg[0] = 4;
            let down = match kind {
                KeyKind::Down => true,
                KeyKind::Up => false,
                KeyKind::FlagsChanged => {
                    modifier_for_vk(vk_code).is_some_and(|bit| modifiers.contains(bit))
                }
            };
            msg[1] = u8::from(down);
            msg[4..8].copy_from_slice(&keysym.to_be_bytes());
            // ARD type-30 encrypts only the credential exchange. Subsequent
            // input uses ordinary RFB key events (as implemented by
            // noVNC/gtk-vnc); do not send the unrelated private 0x10 event.
            stream.write_all(&msg).await
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
    if let Some(c) = unicode {
        return c as u32;
    }
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
    if modifiers.contains(KeyModifiers::SHIFT) {
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

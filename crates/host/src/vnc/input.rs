use super::{ClientMessage, PixelFormat};
use crate::input_sink::{BUTTON_LEFT, BUTTON_RIGHT};
use removent_proto::{KeyModifiers, MouseKind};
use std::io;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

pub(super) async fn read_client_messages(
    stream: &mut tokio::net::tcp::OwnedReadHalf,
    tx: mpsc::Sender<ClientMessage>,
) -> io::Result<()> {
    loop {
        let mut kind = [0u8; 1];
        stream.read_exact(&mut kind).await?;
        match kind[0] {
            0 => {
                let mut data = [0u8; 19];
                stream.read_exact(&mut data).await?;
                let format = PixelFormat {
                    bits_per_pixel: data[3],
                    depth: data[4],
                    big_endian: data[5] != 0,
                    true_colour: data[6] != 0,
                    red_max: u16::from_be_bytes([data[7], data[8]]),
                    green_max: u16::from_be_bytes([data[9], data[10]]),
                    blue_max: u16::from_be_bytes([data[11], data[12]]),
                    red_shift: data[13],
                    green_shift: data[14],
                    blue_shift: data[15],
                };
                let _ = tx.send(ClientMessage::SetPixelFormat(format)).await;
            }
            2 => {
                let mut head = [0u8; 3];
                stream.read_exact(&mut head).await?;
                let count = u16::from_be_bytes([head[1], head[2]]) as usize;
                let mut encodings = vec![0u8; count.saturating_mul(4)];
                stream.read_exact(&mut encodings).await?;
                let _ = tx.send(ClientMessage::Other).await;
            }
            3 => {
                let mut data = [0u8; 9];
                stream.read_exact(&mut data).await?;
                tx.send(ClientMessage::FramebufferRequest {
                    incremental: data[0] != 0,
                    x: u16::from_be_bytes([data[1], data[2]]),
                    y: u16::from_be_bytes([data[3], data[4]]),
                    width: u16::from_be_bytes([data[5], data[6]]),
                    height: u16::from_be_bytes([data[7], data[8]]),
                })
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "VNC writer closed"))?;
            }
            4 => {
                let mut data = [0u8; 7];
                stream.read_exact(&mut data).await?;
                tx.send(ClientMessage::Key {
                    down: data[0] != 0,
                    keysym: u32::from_be_bytes([data[3], data[4], data[5], data[6]]),
                })
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "VNC writer closed"))?;
            }
            5 => {
                let mut data = [0u8; 5];
                stream.read_exact(&mut data).await?;
                tx.send(ClientMessage::Pointer {
                    mask: data[0],
                    x: u16::from_be_bytes([data[1], data[2]]),
                    y: u16::from_be_bytes([data[3], data[4]]),
                })
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "VNC writer closed"))?;
            }
            6 => {
                let mut head = [0u8; 7];
                stream.read_exact(&mut head).await?;
                let len = u32::from_be_bytes([head[3], head[4], head[5], head[6]]) as usize;
                if len > 1024 * 1024 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "VNC cut-text too large",
                    ));
                }
                let mut text = vec![0u8; len];
                stream.read_exact(&mut text).await?;
            }
            150 => {
                // EnableContinuousUpdates: enable(1), x/y/width/height (8).
                // We continue to drive updates from the regular framebuffer
                // request path, but consuming this extension keeps clients
                // such as modern noVNC connected.
                let mut data = [0u8; 9];
                stream.read_exact(&mut data).await?;
                let _ = tx.send(ClientMessage::Other).await;
            }
            248 => {
                // ClientFence: padding(3), flags(4), payload length(1), data.
                let mut head = [0u8; 8];
                stream.read_exact(&mut head).await?;
                let len = head[7] as usize;
                if len > 1024 * 1024 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "VNC fence payload too large",
                    ));
                }
                let mut payload = vec![0u8; len];
                stream.read_exact(&mut payload).await?;
                let _ = tx.send(ClientMessage::Other).await;
            }
            250 => {
                // XVP operation: version, message, and one byte of padding.
                let mut data = [0u8; 3];
                stream.read_exact(&mut data).await?;
                let _ = tx.send(ClientMessage::Other).await;
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unknown VNC message",
                ));
            }
        }
    }
}

pub(super) fn key_for_keysym(keysym: u32) -> Option<(u16, Option<KeyModifiers>)> {
    if (b'a' as u32..=b'z' as u32).contains(&keysym) {
        const LETTERS: [u16; 26] = [
            0x00, 0x0b, 0x08, 0x02, 0x0e, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2e, 0x2d,
            0x1f, 0x23, 0x0c, 0x0f, 0x01, 0x11, 0x20, 0x09, 0x0d, 0x07, 0x10, 0x06,
        ];
        return Some((LETTERS[(keysym - b'a' as u32) as usize], None));
    }
    if (b'A' as u32..=b'Z' as u32).contains(&keysym) {
        const LETTERS: [u16; 26] = [
            0x00, 0x0b, 0x08, 0x02, 0x0e, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2e, 0x2d,
            0x1f, 0x23, 0x0c, 0x0f, 0x01, 0x11, 0x20, 0x09, 0x0d, 0x07, 0x10, 0x06,
        ];
        return Some((
            LETTERS[(keysym - b'A' as u32) as usize],
            Some(KeyModifiers::SHIFT),
        ));
    }
    if (b'1' as u32..=b'9' as u32).contains(&keysym) {
        const DIGITS: [u16; 9] = [0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1a, 0x1c, 0x19];
        return Some((DIGITS[(keysym - b'1' as u32) as usize], None));
    }
    let vk = match keysym {
        0x08 => 0x33,
        0x09 => 0x30,
        0x0d => 0x24,
        0x1b => 0x35,
        0xffff => 0x75,
        0xff50 => 0x73,
        0xff51 => 0x7b,
        0xff52 => 0x7e,
        0xff53 => 0x7c,
        0xff54 => 0x7d,
        0xff55 => 0x74,
        0xff56 => 0x79,
        0xff57 => 0x77,
        0xff63 => 0x72,
        0xffbe => 0x7a,
        0xffbf => 0x78,
        0xffc0 => 0x63,
        0xffc1 => 0x76,
        0xffc2 => 0x60,
        0xffc3 => 0x61,
        0xffc4 => 0x62,
        0xffc5 => 0x64,
        0xffc6 => 0x65,
        0xffc7 => 0x6d,
        0xffc8 => 0x67,
        0xffc9 => 0x6f,
        0xffe1 | 0xffe2 => return Some((0x38, Some(KeyModifiers::SHIFT))),
        0xffe3 | 0xffe4 => return Some((0x3b, Some(KeyModifiers::CONTROL))),
        0xffe9 | 0xffea => return Some((0x3a, Some(KeyModifiers::OPTION))),
        0xffeb | 0xffec => return Some((0x37, Some(KeyModifiers::COMMAND))),
        0xffe5 => return Some((0x39, Some(KeyModifiers::CAPS_LOCK))),
        0x21 => return Some((0x12, Some(KeyModifiers::SHIFT))), // !
        0x40 => return Some((0x13, Some(KeyModifiers::SHIFT))), // @
        0x23 => return Some((0x14, Some(KeyModifiers::SHIFT))), // #
        0x24 => return Some((0x15, Some(KeyModifiers::SHIFT))), // $
        0x25 => return Some((0x17, Some(KeyModifiers::SHIFT))), // %
        0x5e => return Some((0x16, Some(KeyModifiers::SHIFT))), // ^
        0x26 => return Some((0x1a, Some(KeyModifiers::SHIFT))), // &
        0x2a => return Some((0x1c, Some(KeyModifiers::SHIFT))), // *
        0x28 => return Some((0x19, Some(KeyModifiers::SHIFT))), // (
        0x29 => return Some((0x1d, Some(KeyModifiers::SHIFT))), // )
        0x5f => return Some((0x1b, Some(KeyModifiers::SHIFT))), // _
        0x2b => return Some((0x18, Some(KeyModifiers::SHIFT))), // +
        0x7b => return Some((0x21, Some(KeyModifiers::SHIFT))), // {
        0x7d => return Some((0x1e, Some(KeyModifiers::SHIFT))), // }
        0x7c => return Some((0x2a, Some(KeyModifiers::SHIFT))), // |
        0x3a => return Some((0x29, Some(KeyModifiers::SHIFT))), // :
        0x22 => return Some((0x27, Some(KeyModifiers::SHIFT))), // "
        0x3c => return Some((0x2b, Some(KeyModifiers::SHIFT))), // <
        0x3e => return Some((0x2f, Some(KeyModifiers::SHIFT))), // >
        0x3f => return Some((0x2c, Some(KeyModifiers::SHIFT))), // ?
        0x7e => return Some((0x32, Some(KeyModifiers::SHIFT))), // ~
        0x30 => 0x1d,
        0x2d => 0x1b,
        0x3d => 0x18,
        0x5b => 0x21,
        0x5d => 0x1e,
        0x5c => 0x2a,
        0x3b => 0x29,
        0x27 => 0x27,
        0x2c => 0x2b,
        0x2e => 0x2f,
        0x2f => 0x2c,
        0x60 => 0x32,
        0x20 => 0x31,
        _ => return None,
    };
    Some((vk, None))
}

pub(super) fn pointer_button(mask: u8, old: u8, bit: u8) -> Option<MouseKind> {
    if mask & bit != 0 && old & bit == 0 {
        Some(match bit {
            BUTTON_LEFT => MouseKind::LeftDown,
            BUTTON_RIGHT => MouseKind::RightDown,
            _ => MouseKind::MiddleDown,
        })
    } else if mask & bit == 0 && old & bit != 0 {
        Some(match bit {
            BUTTON_LEFT => MouseKind::LeftUp,
            BUTTON_RIGHT => MouseKind::RightUp,
            _ => MouseKind::MiddleUp,
        })
    } else {
        None
    }
}

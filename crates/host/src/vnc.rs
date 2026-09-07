//! Legacy RFB/VNC server for interoperability with Apple Remote Desktop and
//! standard VNC viewers.
//!
//! The compatibility listener is intentionally conservative and disabled by
//! default. Protocol parsing, framebuffer encoding, and input translation
//! live in focused submodules while this facade owns service lifecycle.

use crate::input_sink::InputSink;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const RFB_VERSION: &[u8; 12] = b"RFB 003.008\n";
const SEC_NONE: u8 = 1;
const SEC_VNC_AUTH: u8 = 2;
const MAX_FRAME_BYTES: usize = 1920 * 1080 * 4;
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);

#[derive(Clone)]
pub struct VncConfig {
    pub bind_addr: SocketAddr,
    pub password: String,
    pub input_sink: Option<Arc<dyn InputSink>>,
    pub shutdown: CancellationToken,
    pub session_slot: Arc<tokio::sync::Semaphore>,
}

#[derive(Clone)]
pub(super) struct DisplayTarget {
    pub(super) id: u64,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) capture_width: u32,
    pub(super) capture_height: u32,
}

#[derive(Clone, Copy)]
pub(super) struct PixelFormat {
    pub(super) bits_per_pixel: u8,
    pub(super) depth: u8,
    pub(super) big_endian: bool,
    pub(super) true_colour: bool,
    pub(super) red_max: u16,
    pub(super) green_max: u16,
    pub(super) blue_max: u16,
    pub(super) red_shift: u8,
    pub(super) green_shift: u8,
    pub(super) blue_shift: u8,
}

pub(super) struct Frame {
    pub(super) data: Vec<u8>,
    pub(super) width: u32,
    pub(super) height: u32,
}

pub(super) enum ClientMessage {
    SetPixelFormat(PixelFormat),
    FramebufferRequest {
        incremental: bool,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
    },
    Pointer {
        mask: u8,
        x: u16,
        y: u16,
    },
    Key {
        down: bool,
        keysym: u32,
    },
    Other,
}

pub(super) fn default_pixel_format() -> PixelFormat {
    PixelFormat {
        bits_per_pixel: 32,
        depth: 24,
        big_endian: false,
        true_colour: true,
        red_max: 255,
        green_max: 255,
        blue_max: 255,
        red_shift: 16,
        green_shift: 8,
        blue_shift: 0,
    }
}

impl PixelFormat {
    pub(super) fn supported(self) -> bool {
        self.true_colour
            && matches!(self.bits_per_pixel, 16 | 32)
            && self.depth > 0
            && self.depth <= self.bits_per_pixel
            && self.red_max > 0
            && self.green_max > 0
            && self.blue_max > 0
            && channel_fits(self.red_max, self.red_shift, self.bits_per_pixel)
            && channel_fits(self.green_max, self.green_shift, self.bits_per_pixel)
            && channel_fits(self.blue_max, self.blue_shift, self.bits_per_pixel)
    }
}

fn channel_fits(max: u16, shift: u8, bits_per_pixel: u8) -> bool {
    let width = u16::from(u16::BITS as u8 - max.leading_zeros() as u8);
    u16::from(shift) + width <= u16::from(bits_per_pixel)
}

mod framebuffer;
mod input;
mod protocol;
mod writer;

#[cfg(test)]
mod tests;

#[cfg(test)]
use framebuffer::send_frame_message;
#[cfg(test)]
use input::key_for_keysym;
#[cfg(test)]
use protocol::vnc_response;

/// Run the VNC listener until the host service is stopped.
pub async fn serve_vnc(cfg: VncConfig) -> io::Result<()> {
    let listener = TcpListener::bind(cfg.bind_addr).await?;
    if cfg.password.is_empty() {
        tracing::warn!("RFB/VNC listener has no password; connections are unauthenticated");
    }
    tracing::info!(addr = ?cfg.bind_addr, "RFB/VNC compatibility listener started");
    let mut clients = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = cfg.shutdown.cancelled() => break,
            _ = clients.join_next(), if !clients.is_empty() => continue,
            accepted = listener.accept() => {
                let (stream, peer) = accepted?;
                let Ok(permit) = cfg.session_slot.clone().try_acquire_owned() else {
                    tracing::debug!(?peer, "VNC connection rejected while host is busy");
                    continue;
                };
                let cfg = cfg.clone();
                clients.spawn(async move {
                    if let Err(e) = serve_client(stream, cfg, permit).await {
                        tracing::debug!(?peer, err = %e, "VNC client disconnected");
                    }
                });
            }
        }
    }
    clients.shutdown().await;
    tracing::info!("RFB/VNC compatibility listener stopped");
    Ok(())
}

async fn serve_client(
    mut stream: TcpStream,
    cfg: VncConfig,
    _permit: tokio::sync::OwnedSemaphorePermit,
) -> io::Result<()> {
    let target = tokio::select! {
        _ = cfg.shutdown.cancelled() => {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "VNC service stopped"));
        }
        result = tokio::time::timeout(
            HANDSHAKE_TIMEOUT,
            protocol::perform_handshake(&mut stream, cfg.password.as_bytes()),
        ) => result
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "VNC handshake timed out"))??,
    };

    let (raw_tx, mut raw_rx) = removent_core::latest::channel::<(Vec<u8>, i64)>();
    let (frame_tx, frame_rx) = removent_core::latest::channel::<Frame>();
    let capture = removent_media_capture::start_display_capture(
        target.id as u32,
        target.capture_width,
        target.capture_height,
        raw_tx,
        None,
    )
    .map_err(|e| io::Error::other(e.to_string()))?;
    let frame_target = target.clone();
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(async move {
        while let Some((data, _pts)) = raw_rx.recv().await {
            let frame = Frame {
                data,
                width: frame_target.capture_width,
                height: frame_target.capture_height,
            };
            if frame_tx.send(frame).is_err() {
                break;
            }
        }
    });
    if let Some(input) = cfg.input_sink.as_ref() {
        input.set_capture_dims(target.capture_width, target.capture_height);
    }

    let (msg_tx, msg_rx) = mpsc::channel::<ClientMessage>(16);
    let (mut read_half, write_half) = stream.into_split();
    tasks.spawn(async move {
        let _ = input::read_client_messages(&mut read_half, msg_tx).await;
    });
    let writer_result = writer::write_frames(
        write_half,
        frame_rx,
        msg_rx,
        target,
        cfg.input_sink.clone(),
        cfg.shutdown.clone(),
    )
    .await;
    tasks.shutdown().await;
    drop(capture);
    writer_result
}

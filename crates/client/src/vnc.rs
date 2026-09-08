//! Standard RFB/VNC client used for interoperability with macOS Screen Sharing
//! and Apple Remote Desktop servers.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::net::TcpStream;
use tokio::sync::{RwLock, mpsc};
use tracing::Instrument;

use crate::DecodedFrame;

const RFB_VERSION: &[u8; 12] = b"RFB 003.008\n";
const ARD_VERSION: &[u8; 12] = b"RFB 003.889\n";
const SEC_NONE: u8 = 1;
const SEC_VNC_AUTH: u8 = 2;
const SEC_ARD: u8 = 30;
const SEC_ARD_MACOS: u8 = 35;
const MAX_NAME: usize = 1024 * 1024;
const MAX_PIXELS: usize = 16 * 1024 * 1024;
const ARD_SERVER_FLAG_SESSION_SELECT: u32 = 0x04;
const ARD_SESSION_CMD_REQUEST_CONSOLE: u8 = 0;
const ARD_SESSION_CMD_CONNECT_CONSOLE: u8 = 1;
const ARD_SESSION_CMD_CONNECT_VIRTUAL: u8 = 2;
const ARD_SESSION_STATUS_GRANTED: u32 = 0;
const ARD_SESSION_STATUS_PENDING: u32 = 2;
const ARD_SESSION_STATUS_PENDING_ALT: u32 = 3;
const ARD_SESSION_STATUS_GRANTED_AFTER_PENDING: u32 = 4;

#[derive(Debug, thiserror::Error)]
pub enum VncError {
    #[error("VNC network: {0}")]
    Io(#[from] io::Error),
    #[error("VNC protocol: {0}")]
    Protocol(String),
    #[error("VNC timed out during {stage} after {seconds} seconds")]
    Timeout { stage: &'static str, seconds: u64 },
    #[error(
        "VNC authentication failed: check the remote Mac account name and password (or the dedicated VNC password)"
    )]
    Authentication,
}

/// Commands accepted by an established VNC session. Reusing `ControlMsg`
/// keeps the viewer and engine input path identical for RVP and RFB.
pub struct VncSession {
    pub cmd_tx: InputSender,
    pub decoded_bgra_rx: removent_core::latest::Receiver<DecodedFrame>,
    pub stats: Arc<VncStats>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

#[derive(Default)]
pub struct VncStats {
    pub(super) received_bytes: AtomicU64,
    pub(super) received_frames: AtomicU64,
    pub(super) decode_us: AtomicU64,
    pub(super) update_us: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct VncSnapshot {
    pub received_bytes: u64,
    pub received_frames: u64,
    pub decode_us: u64,
    /// Time to receive and process the latest update, excluding server idle time.
    pub update_us: u64,
}

impl VncStats {
    pub fn snapshot(&self) -> VncSnapshot {
        VncSnapshot {
            received_bytes: self.received_bytes.load(Ordering::Relaxed),
            received_frames: self.received_frames.load(Ordering::Relaxed),
            decode_us: self.decode_us.load(Ordering::Relaxed),
            update_us: self.update_us.load(Ordering::Relaxed),
        }
    }
}

impl Drop for VncSession {
    fn drop(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct PixelFormat {
    pub(super) bits_per_pixel: u8,
    pub(super) depth: u8,
    pub(super) big_endian: bool,
    pub(super) red_max: u16,
    pub(super) green_max: u16,
    pub(super) blue_max: u16,
    pub(super) red_shift: u8,
    pub(super) green_shift: u8,
    pub(super) blue_shift: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct FrameSize {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) authoritative: bool,
}

pub(super) enum FrameSignal {
    Updated,
    Closed,
}

mod auth;
mod framebuffer;
mod input;
mod queue;

pub use queue::{InputSender, InputSnapshot};

#[cfg(test)]
mod tests;

#[cfg(test)]
use auth::{choose_security_type, copy_c_string, fixed_be_bytes, negotiated_version, vnc_response};
#[cfg(test)]
use framebuffer::scale_component;
#[cfg(test)]
use input::{keysym_for_key, rfb_button_mask};

/// Connect to a standard RFB server. `password` is the traditional VNC
/// password (only its first eight bytes are significant).
pub async fn connect_vnc(addr: SocketAddr, password: &str) -> Result<VncSession, VncError> {
    connect_vnc_with_credentials(addr, "", password).await
}

/// Connect to a standard VNC or Apple Remote Desktop (RFB 003.889) server.
/// Apple authentication requires both a macOS account name and password.
pub async fn connect_vnc_with_credentials(
    addr: SocketAddr,
    username: &str,
    password: &str,
) -> Result<VncSession, VncError> {
    connect_vnc_with_progress(addr, username, password, None).await
}

pub async fn connect_vnc_with_progress(
    addr: SocketAddr,
    username: &str,
    password: &str,
    progress: Option<&crate::connection::ConnectionProgress>,
) -> Result<VncSession, VncError> {
    crate::connection::report_progress(progress, crate::connection::ConnectionStage::Connecting);
    let mut stream = with_timeout("TCP connection", 8, async {
        Ok(TcpStream::connect(addr).await?)
    })
    .await?;
    stream.set_nodelay(true)?;
    let (width, height, _server_format, apple_ard) =
        auth::handshake(&mut stream, username, password.as_bytes(), progress).await?;
    let (read_half, mut write_half) = stream.into_split();
    let dimensions = Arc::new(RwLock::new(FrameSize {
        width,
        height,
        authoritative: width > 0 && height > 0,
    }));

    let format = framebuffer::requested_pixel_format();
    framebuffer::write_pixel_format(&mut write_half).await?;
    framebuffer::write_set_encodings(&mut write_half).await?;
    if width > 0 && height > 0 {
        framebuffer::write_framebuffer_request(&mut write_half, false, width, height).await?;
    } else if apple_ard {
        framebuffer::write_framebuffer_request(
            &mut write_half,
            false,
            u16::MAX.into(),
            u16::MAX.into(),
        )
        .await?;
    }

    let (cmd_tx, cmd_rx) = queue::channel();
    let (frame_tx, frame_rx) = removent_core::latest::channel();
    let (signal_tx, signal_rx) = mpsc::channel(4);
    let stats = Arc::new(VncStats::default());
    let reader_stats = stats.clone();
    // Independently scheduled tasks keep large framebuffer work from blocking
    // input. The supervisor owns both: EOF, writer failure, and session drop
    // still close the entire connection (including any pending read).
    let task = tokio::spawn(
        async move {
            let mut tasks = tokio::task::JoinSet::new();
            tasks.spawn(
                framebuffer::read_frames(
                    read_half,
                    frame_tx,
                    signal_tx,
                    dimensions.clone(),
                    format,
                    reader_stats,
                )
                .in_current_span(),
            );
            tasks.spawn(
                input::write_commands(write_half, cmd_rx, signal_rx, dimensions, apple_ard)
                    .in_current_span(),
            );
            tasks.join_next().await;
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        }
        .in_current_span(),
    );
    Ok(VncSession {
        cmd_tx,
        decoded_bgra_rx: frame_rx,
        stats,
        tasks: vec![task],
    })
}

/// Bound individual handshake stages so callers can distinguish an unreachable
/// server from an authentication or desktop-attachment stall.
async fn with_timeout<T>(
    stage: &'static str,
    seconds: u64,
    future: impl std::future::Future<Output = Result<T, VncError>>,
) -> Result<T, VncError> {
    tokio::time::timeout(std::time::Duration::from_secs(seconds), future)
        .await
        .map_err(|_| VncError::Timeout { stage, seconds })?
}

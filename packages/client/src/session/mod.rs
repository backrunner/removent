//! Controller-side session engine: connect, pairing, negotiation, media receive, and
//! stats pump.
//!
//! Sequencing mirrors the host engine (protocol.md §4–5); decoded media output is
//! delivered over channels to the render layer or a test collector. Fast reconnect
//! recovery: [`quick_resume`].

use crate::jitter::{AudioPacketIn, JitterBuffer, PopOutcome};
use crate::video_feedback::VideoFeedback;
use futures::{SinkExt, StreamExt};
use removent_core::{ClipSyncState, DeviceIdentity};
use removent_media_codec::{AudioDecoder, VideoDecoder, extract_param_sets};
use removent_net::{
    ControlItem, ControlSink, ControlSource, PairingMsg, RvpConnection, client_begin,
    client_confirm_check, client_verify,
};
use removent_proto::{
    Caps, ControlMsg, EndReason, HandshakeClient, Hello, NegotiateAck, parse_audio_header,
    parse_video_header,
};

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("net: {0}")]
    Net(#[from] removent_net::NetError),
    #[error("timeout waiting for {0}")]
    Timeout(&'static str),
    #[error("rejected: {0}")]
    Rejected(String),
    #[error("pairing failed: {0}")]
    Pairing(String),
}

/// Connection configuration.
pub struct ClientConfig {
    pub device_name: String,
    pub caps: Caps,
    /// Local clipboard bridge (NSPasteboard on real machines; an in-memory impl in tests).
    pub local_clip: Option<std::sync::Arc<dyn removent_core::TextClipboard>>,
}

/// User input callback: returns the PIN transcribed by the user.
pub type PinInput = oneshot::Receiver<String>;

/// Fired only when the session actually needs a PIN (unknown peer, pairing
/// initiated): the UI shows the PIN prompt at this point, not at connect time.
/// The callback receives the sender half used to deliver the transcribed PIN.
pub type PinRequest = Box<dyn FnOnce(oneshot::Sender<String>) + Send>;

/// Minimum interval between KeyframeRequests (protocol.md §7.1).
const KEYFRAME_MIN_INTERVAL: Duration = Duration::from_millis(500);

/// A decoded BGRA frame (consumed by the renderer; carries frame dimensions to support
/// resolution hot-switching).
#[derive(Debug, Clone)]
pub struct DecodedFrame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub pts_us: i64,
}

/// An established client session.
pub struct ClientSession {
    pub conn: RvpConnection,
    /// App layer → peer control-message egress (mouse/keyboard/clipboard/keyframe/end).
    pub cmd_tx: mpsc::Sender<ControlMsg>,
    /// Ordered UI input: coalesces adjacent motion while preserving transitions
    /// and the final pointer position during network backpressure.
    pub input_tx: crate::InputSender,
    pub negotiated: NegotiateAck,
    /// Most recently received quick-resume token (the host rotates it every session, §7.4).
    pub resume_token: Arc<Mutex<Option<[u8; 16]>>>,
    /// Decoded BGRA frames (consumed by the renderer).
    pub decoded_bgra_rx: removent_core::latest::Receiver<DecodedFrame>,
    /// Decoded PCM (consumed by playback).
    pub decoded_pcm_rx: mpsc::Receiver<Vec<i16>>,
    /// Most recent host quality directive (bitrate_kbps, fps, scale).
    pub last_quality: Arc<Mutex<Option<(u32, u8, f32)>>>,
    /// Time of the last outbound KeyframeRequest (rate limiting).
    last_kf: Arc<Mutex<Option<Instant>>>,
    /// Background task handles (aborted on close/drop of the session).
    tasks: Vec<tokio::task::JoinHandle<()>>,
    /// Completed after the control pump flushes SessionEnd and tears down media.
    closed: oneshot::Receiver<()>,
    clean_end: Arc<std::sync::atomic::AtomicBool>,
}

impl ClientSession {
    /// Stream EOF/errors also interrupt a session, even if pump cleanup then
    /// locally closes QUIC. Preserve explicit SessionEnd separately.
    pub fn was_interrupted(&self) -> bool {
        !self.clean_end.load(std::sync::atomic::Ordering::SeqCst)
            && !matches!(
                self.conn.inner().close_reason(),
                Some(removent_net::quinn::ConnectionError::ApplicationClosed(_))
            )
    }

    /// Request a keyframe (quality convergence / stall recovery); rate-limited to a
    /// 500ms minimum interval (protocol.md §7.1).
    pub async fn request_keyframe(&self) -> Result<(), ConnectError> {
        {
            let mut last = self.last_kf.lock().unwrap();
            let now = Instant::now();
            if last.is_some_and(|t| now.duration_since(t) < KEYFRAME_MIN_INTERVAL) {
                return Ok(());
            }
            *last = Some(now);
        }
        self.send(ControlMsg::KeyframeRequest).await
    }

    /// Current quick-resume token (if any). Consume it with [`quick_resume`] when reconnecting.
    pub fn current_resume_token(&self) -> Option<[u8; 16]> {
        *self.resume_token.lock().unwrap()
    }

    /// Proactive close: send SessionEnd, then terminate all background tasks and close
    /// the connection.
    pub async fn close(mut self) -> Result<(), ConnectError> {
        // Enqueuing SessionEnd is not a flush: wait for the pump to send it
        // before aborting tasks. A stalled peer must not block close forever.
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            let _ = self
                .cmd_tx
                .send(ControlMsg::SessionEnd {
                    reason: EndReason::ClientClosed,
                })
                .await;
            let _ = (&mut self.closed).await;
        })
        .await;
        self.teardown();
        Ok(())
    }

    fn teardown(&mut self) {
        for t in self.tasks.drain(..) {
            t.abort();
        }
        self.conn
            .inner()
            .close(removent_net::quinn::VarInt::from_u32(0), b"session closed");
    }

    pub async fn send_input_mouse(
        &self,
        display_id: u64,
        x_px: f32,
        y_px: f32,
        buttons: u8,
        kind: removent_proto::MouseKind,
    ) -> Result<(), ConnectError> {
        self.send_input(ControlMsg::MouseEvent {
            display_id,
            x_px,
            y_px,
            buttons,
            kind,
        })
        .await
    }

    pub async fn send_input_key(
        &self,
        vk_code: u16,
        modifiers: removent_proto::KeyModifiers,
        kind: removent_proto::KeyKind,
        unicode: Option<char>,
    ) -> Result<(), ConnectError> {
        self.send_input(ControlMsg::KeyEvent {
            vk_code,
            modifiers,
            kind,
            unicode,
        })
        .await
    }

    pub async fn send_input_scroll(
        &self,
        display_id: u64,
        dx_mm: f32,
        dy_mm: f32,
        phase: removent_proto::ScrollPhase,
    ) -> Result<(), ConnectError> {
        self.send_input(ControlMsg::ScrollEvent {
            display_id,
            dx_mm,
            dy_mm,
            phase,
        })
        .await
    }

    pub async fn send_clipboard_text(&self, seq: u32, text: &str) -> Result<(), ConnectError> {
        self.send(ControlMsg::ClipboardSync {
            seq,
            format: removent_proto::ClipFormat::TextUtf8,
            data: text.as_bytes().to_vec(),
        })
        .await
    }

    /// Send a stats report (input for adaptive quality adjustment).
    pub async fn send_stats(
        &self,
        rtt_ms: f32,
        loss_pct: f32,
        recv_kbps: u32,
        jitter_ms: f32,
    ) -> Result<(), ConnectError> {
        self.send(ControlMsg::StatsReport {
            rtt_ms,
            loss_pct,
            recv_kbps,
            jitter_ms,
            decode_ms: 0.0,
            render_fps: 0.0,
        })
        .await
    }

    pub async fn end_session(&self) -> Result<(), ConnectError> {
        self.send(ControlMsg::SessionEnd {
            reason: EndReason::ClientClosed,
        })
        .await
    }

    async fn send(&self, msg: ControlMsg) -> Result<(), ConnectError> {
        self.cmd_tx
            .send(msg)
            .await
            .map_err(|_| ConnectError::Rejected("session closed".into()))
    }

    async fn send_input(&self, msg: ControlMsg) -> Result<(), ConnectError> {
        self.input_tx
            .send(msg)
            .await
            .map_err(|_| ConnectError::Rejected("session closed".into()))
    }
}

impl Drop for ClientSession {
    fn drop(&mut self) {
        self.teardown();
    }
}

mod control;
mod handshake;
mod media;
mod pairing;
#[cfg(test)]
mod tests;

use control::build_session;
pub use handshake::connect_session;
#[cfg(test)]
use handshake::expect_msg;
use media::{AbortOnDrop, spawn_media_loops};
pub use pairing::quick_resume;
use pairing::{hex_encode, platform_version, spawn_pairing_initiator};

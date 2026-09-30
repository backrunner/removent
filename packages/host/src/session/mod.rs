//! Host-side session engine: admission, pairing, negotiation, and media send loops.
//!
//! Media sources are injected via tokio mpsc channels (either the SCK capture
//! implementation or a test synthesizer), so the engine does not depend on TCC
//! permissions and can be verified headless. Message sequencing: protocol.md §4–5.

use crate::admission::{AdmissionDecision, decide};
use crate::frame_dedup::FrameDeduplicator;
use crate::input_sink::InputSink;
use futures::{SinkExt, StreamExt};
use rand::RngCore;
use removent_core::{
    AdmissionMode, ClipSyncState, DeviceIdentity, PeerRecord, PeersStore, TextClipboard,
    adapt::{AdaptationController, QualityState, Sample},
};
use removent_media_codec::VideoEncoder;
use removent_net::{ControlItem, ControlSink, ControlSource, RvpConnection};
use removent_proto::{
    AudioPacketHeader, Caps, CodecId, ControlMsg, HandshakeServer, KeyKind, MouseKind, Negotiate,
    NegotiateAck, PROTO_VERSION, RESUME_WINDOW_SECS, ScrollPhase, VideoParams, build_audio_packet,
    build_video_frame,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Error)]
pub enum HostError {
    #[error("net: {0}")]
    Net(#[from] removent_net::NetError),
    #[error("codec: {0}")]
    Codec(#[from] removent_media_codec::VideoError),
    #[error("rejected: {0}")]
    Rejected(String),
    #[error("timeout waiting for {0}")]
    Timeout(&'static str),
}

/// User interaction callbacks (implemented by the UI or by test auto-responders).
pub type PromptFuture = std::pin::Pin<Box<dyn Future<Output = bool> + Send>>;

pub struct HostInteractions {
    /// Show the PIN to the machine owner (non-blocking).
    pub show_pairing_pin: Box<dyn FnOnce(String) + Send>,
    /// Admission prompt: asynchronously awaits the user's ruling.
    pub admission_prompt: Box<dyn FnOnce(String, String) -> PromptFuture + Send>,
}

impl HostInteractions {
    /// For tests: forwards the PIN over a channel and auto-allows admission.
    pub fn auto_allow_with_pin_tx(pin_tx: oneshot::Sender<String>) -> Self {
        Self {
            show_pairing_pin: Box::new(move |pin| {
                let _ = pin_tx.send(pin);
            }),
            admission_prompt: Box::new(|_, _| {
                Box::pin(async { true }) as std::pin::Pin<Box<dyn Future<Output = bool> + Send>>
            }),
        }
    }
}

/// Host-side configuration.
pub struct HostConfig {
    pub audio_available: bool,
    /// LoginWindow cannot display pairing or capability-expansion prompts.
    pub preapproved_only: bool,
    pub device_name: String,
    pub admission: AdmissionMode,
    pub video_bitrate_kbps: u32,
    pub video_fps: u8,
    /// Real machines pass RealInputSink; tests pass a recorder.
    pub input_sink: Option<Arc<dyn InputSink>>,
    /// Local clipboard bridge (NSPasteboard on real machines; an in-memory impl in tests).
    pub local_clip: Option<Arc<dyn TextClipboard>>,
}

impl HostConfig {
    fn available_caps(&self, mut caps: Caps) -> Caps {
        caps.audio &= self.audio_available;
        caps.input &= self.input_sink.is_some();
        caps.clipboard &= self.local_clip.is_some();
        // No file-transfer implementation is offered by this host.
        caps.file = false;
        caps
    }
}

/// Media sources (host-side capture implementation or test injection).
#[derive(Clone)]
pub struct HostMediaFeeds {
    /// BGRA frames (stride=width*4) + pts in microseconds.
    pub video_tx: removent_core::latest::Sender<(Vec<u8>, i64)>,
    /// 10ms stereo PCM frames with source presentation timestamps.
    pub audio_tx: mpsc::Sender<removent_media_capture::AudioFrame>,
}

/// Session handle produced once negotiation completes.
type SessionChannels = (
    mpsc::Sender<ControlMsg>,
    mpsc::Receiver<ControlMsg>,
    Arc<std::sync::Mutex<AdaptationController>>,
    mpsc::Sender<()>,
    mpsc::Receiver<()>,
    tokio::sync::watch::Sender<QualityState>,
    tokio::sync::watch::Receiver<QualityState>,
);

fn build_session_channels(bitrate_kbps: u32, fps: u8, width: u32, height: u32) -> SessionChannels {
    let (cmd_tx, cmd_rx) = mpsc::channel::<ControlMsg>(64);
    let (kf_tx, kf_rx) = mpsc::channel::<()>(4);
    let controller = Arc::new(std::sync::Mutex::new(
        AdaptationController::with_dimensions(
            bitrate_kbps,
            fps,
            removent_core::QualityPreset::Auto,
            width,
            height,
        ),
    ));
    let (quality_tx, quality_rx) = tokio::sync::watch::channel(controller.lock().unwrap().state());
    (
        cmd_tx, cmd_rx, controller, kf_tx, kf_rx, quality_tx, quality_rx,
    )
}

/// Start the clipboard poller and return the session-level shared state (the pump side
/// writes suppress into the same singleton to prevent echo loops).
/// The polling task terminates when the session is cancelled.
fn spawn_clip_poller(
    clip: Arc<dyn TextClipboard>,
    cmd_tx: mpsc::Sender<ControlMsg>,
    cancel: &CancellationToken,
) -> Arc<ClipSyncState> {
    let state = Arc::new(ClipSyncState::new(clip.as_ref()));
    let handle = removent_core::spawn_clipboard_poller(clip, state.clone(), cmd_tx, 250);
    let cancel = cancel.clone();
    tokio::spawn(async move {
        cancel.cancelled().await;
        handle.abort();
    });
    state
}

pub struct EstablishedSession {
    pub peer_fp_hex: String,
    pub ack: NegotiateAck,
    /// The quick-resume token issued for this session (or after rotation).
    pub resume_token: [u8; 16],
    /// App layer → peer control-message egress (SessionEnd, custom, etc.).
    pub cmd_tx: mpsc::Sender<ControlMsg>,
    /// Adaptation controller (used inside the pump; tests may read its state).
    pub controller: Arc<std::sync::Mutex<AdaptationController>>,
    /// Keyframe-request injection endpoint (forwarded to the video loop).
    pub keyframe_req_tx: mpsc::Sender<()>,
    /// Complete quality state; watch delivery cannot lose the latest decision.
    pub quality_tx: tokio::sync::watch::Sender<QualityState>,
    /// Capability set requested by the peer (the control pump trims input
    /// injection/clipboard writes accordingly).
    pub peer_caps: Caps,
    /// Session-level clipboard sync state (shared with the poller; None when there is
    /// no local clipboard).
    pub clip_state: Option<Arc<ClipSyncState>>,
    /// Session-level stop token: cancelled automatically when the control pump exits,
    /// bringing down the media loops and poller with it.
    pub cancel: CancellationToken,
}

// ---------------- resume token registry ----------------

mod audio;
mod control;
mod handshake;
mod resume;
#[cfg(test)]
mod tests;
mod video;

pub use audio::spawn_audio_loop;
pub use control::{ControlPumpDeps, TokenBucket, spawn_control_pump};
pub use handshake::serve_connection;
#[cfg(test)]
use resume::{PAIRING_BEGIN_MIN_INTERVAL, PairingBeginLimiter, resume_store};
use resume::{
    invalidate_resume, now_unix, pairing_begin_limiter, remember_resume, validate_resume_full,
};
pub use resume::{refresh_resume, validate_resume};
use video::newest_queued;
pub use video::{MAX_CAPTURE_H, MAX_CAPTURE_W, fit_capture_dims, spawn_video_loop};
#[cfg(test)]
use video::{mark_submitted_frame_sent, scaled_dims};

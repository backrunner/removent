//! RVP control messages and negotiation structures (docs/design/protocol.md §5–6).

use bitflags::bitflags;
use serde::{Deserialize, Serialize};

pub use crate::constants::CodecId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Caps {
    pub video: bool,
    pub audio: bool,
    pub input: bool,
    pub clipboard: bool,
    pub file: bool,
}

impl Caps {
    pub fn all() -> Self {
        Self {
            video: true,
            audio: true,
            input: true,
            clipboard: true,
            file: true,
        }
    }
    pub fn none() -> Self {
        Self::default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub id: u64,
    pub w_px: u32,
    pub h_px: u32,
    pub scale: f32,
    pub dpi: u32,
    pub is_main: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectReason {
    Denied,
    Busy,
    Capability,
    PermissionLost,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndReason {
    ClientClosed,
    HostClosed,
    Shutdown,
    InternalError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    VersionMismatch,
    Unauthorized,
    Busy,
    DisplayGone,
    PermissionLost,
    Internal,
}

impl ErrorCode {
    pub fn message(self) -> &'static str {
        match self {
            ErrorCode::VersionMismatch => {
                "protocol versions are incompatible; please upgrade both ends to the same version and retry"
            }
            ErrorCode::Unauthorized => "the peer denied this connection request",
            ErrorCode::Busy => "the peer is occupied by another session",
            ErrorCode::DisplayGone => "the target display has been disconnected",
            ErrorCode::PermissionLost => "the peer is missing required system permissions",
            ErrorCode::Internal => "internal error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseKind {
    Moved,
    LeftDown,
    LeftUp,
    RightDown,
    RightUp,
    MiddleDown,
    MiddleUp,
    LeftDragged,
    RightDragged,
    MiddleDragged,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ScrollPhase {
    Began,
    Changed,
    Ended,
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
    pub struct KeyModifiers: u8 {
        const CAPS_LOCK = 1 << 0;
        const SHIFT = 1 << 1;
        const CONTROL = 1 << 2;
        const OPTION = 1 << 3;
        const COMMAND = 1 << 4;
        const NUMPAD = 1 << 5;
        const FUNCTION = 1 << 6;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyKind {
    Down,
    Up,
    FlagsChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipFormat {
    TextUtf8,
    Rtf,
    Html,
    Png,
    FileRefs,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VideoParams {
    pub codec: CodecId,
    pub max_fps: u8,
    pub max_bitrate_kbps: u32,
    pub initial_scale: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioParams {
    pub enabled: bool,
    pub sample_rate: u32,
    pub channels: u8,
    pub frame_ms: u16,
    pub bitrate_kbps: u32,
}

impl Default for AudioParams {
    fn default() -> Self {
        Self {
            enabled: true,
            sample_rate: 48_000,
            channels: 2,
            frame_ms: 10,
            bitrate_kbps: 64,
        }
    }
}

/// First message the client sends on the control stream (MAGIC check + version negotiation + self-introduction).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandshakeClient {
    pub magic: [u8; 4],
    pub proto_version: u16,
    pub feature_bits: u64,
    pub hello: Hello,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub app_version: String,
    pub device_name: String,
    pub os_version: String,
    pub caps: Caps,
    /// Fast-resume token (protocol.md §7.4); None on first connection.
    pub resume_token: Option<[u8; 16]>,
}

/// Server's reply to the handshake.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandshakeServer {
    pub proto_version: u16,
    pub feature_bits: u64,
    pub device_name: String,
    /// Honest resume verdict (protocol.md §7.4): None when the client presented no
    /// token, Some(true) when it validated (quick-resume fast path), Some(false)
    /// when it was rejected (the client falls back to full negotiation).
    pub resume_accepted: Option<bool>,
    /// This attempt requires no credential proof: known identity in pairing mode,
    /// no-auth mode, or an immediate Busy rejection. Password/OTP always set false.
    pub peer_known: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Negotiate {
    pub displays: Vec<DisplayInfo>,
    pub selected_display: u64,
    pub video: VideoParams,
    pub audio: AudioParams,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NegotiateAck {
    pub video: VideoParams,
    pub audio: AudioParams,
    /// Fast-resume token issued by the host when negotiation completes.
    pub resume_token: Option<[u8; 16]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ControlMsg {
    SessionRequest {
        caps: Caps,
    },
    SessionAccept,
    SessionReject {
        reason: RejectReason,
    },
    SessionEnd {
        reason: EndReason,
    },
    Ping {
        ts_us: u64,
    },
    Pong {
        ts_us: u64,
    },
    DisplayListUpdate {
        displays: Vec<DisplayInfo>,
    },
    SelectDisplay {
        id: u64,
    },
    QualityControl {
        bitrate_kbps: u32,
        fps: u8,
        scale: f32,
    },
    KeyframeRequest,
    StatsReport {
        rtt_ms: f32,
        loss_pct: f32,
        recv_kbps: u32,
        jitter_ms: f32,
        decode_ms: f32,
        render_fps: f32,
    },
    MouseEvent {
        display_id: u64,
        x_px: f32,
        y_px: f32,
        buttons: u8,
        kind: MouseKind,
    },
    ScrollEvent {
        display_id: u64,
        dx_mm: f32,
        dy_mm: f32,
        phase: ScrollPhase,
    },
    KeyEvent {
        vk_code: u16,
        modifiers: KeyModifiers,
        kind: KeyKind,
        unicode: Option<char>,
    },
    CursorShape {
        cursor_id: u32,
        w: u16,
        h: u16,
        hotspot_x: u16,
        hotspot_y: u16,
        rgba: Vec<u8>,
    },
    CursorPosition {
        x_px: f32,
        y_px: f32,
        visible: bool,
    },
    ClipboardSync {
        seq: u32,
        format: ClipFormat,
        data: Vec<u8>,
    },
    ClipboardAck {
        seq: u32,
    },
    FileOffer {
        transfer_id: u64,
        name: String,
        size: u64,
        sha256: [u8; 32],
        to_remote: bool,
    },
    FileAccept {
        transfer_id: u64,
        offset: u64,
    },
    FileProgress {
        transfer_id: u64,
        bytes_done: u64,
    },
    FileComplete {
        transfer_id: u64,
        sha256_ok: bool,
    },
    FileCancel {
        transfer_id: u64,
        reason: String,
    },
    Error {
        code: ErrorCode,
    },

    // ---- Added in v0.1 (protocol discipline: append only at the tail, protocol.md §8) ----
    /// host → client: authoritative proposal of session media parameters.
    NegotiateOffer {
        n: Box<Negotiate>,
    },
    /// client → host: confirmed final parameters (including the host-issued resume token).
    NegotiateReply {
        ack: Box<NegotiateAck>,
    },
    /// Client → host: geometry currently displayed by the input-producing viewer.
    /// Ordered with MouseEvent so resizing cannot reinterpret old coordinates.
    FrameGeometry {
        width: u32,
        height: u32,
    },
}

/// Host-selected native connection authentication. Append variants for wire compatibility.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthenticationMode {
    PairingCode,
    Password,
    Otp,
    None,
    #[default]
    SystemAccount,
}
impl AuthenticationMode {
    pub fn valid_input(self, value: &str) -> bool {
        match self {
            Self::Password => !value.is_empty() && value.len() <= 1024,
            Self::PairingCode | Self::Otp => {
                value.len() == 6 && value.bytes().all(|b| b.is_ascii_digit())
            }
            Self::None => value.is_empty(),
            Self::SystemAccount => SystemCredentials::decode(value).is_some(),
        }
    }
}

/// Ephemeral system credentials. Never persist or include their values in logs.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemCredentials {
    pub username: String,
    pub password: String,
}
impl std::fmt::Debug for SystemCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SystemCredentials([redacted])")
    }
}
impl SystemCredentials {
    pub fn valid(&self) -> bool {
        !self.username.is_empty()
            && self.username.len() <= 256
            && !self.username.chars().any(char::is_control)
            && !self.password.is_empty()
            && self.password.len() <= 1024
            && !self.password.contains('\0')
    }
    /// The existing controller/FFI credential channel transports a JSON envelope.
    pub fn encode(&self) -> String {
        serde_json::to_string(self).expect("credential serialization")
    }
    pub fn decode(value: &str) -> Option<Self> {
        if value.len() > 8192 {
            return None;
        }
        let credentials: Self = serde_json::from_str(value).ok()?;
        credentials.valid().then_some(credentials)
    }
}

#[cfg(test)]
mod system_credentials_tests {
    use super::*;
    #[test]
    fn system_credentials_are_bounded_and_redacted() {
        assert_eq!(
            AuthenticationMode::default(),
            AuthenticationMode::SystemAccount
        );
        let mut input = SystemCredentials {
            username: "owner".into(),
            password: "private-login-password".into(),
        };
        assert!(AuthenticationMode::SystemAccount.valid_input(&input.encode()));
        assert!(!format!("{input:?}").contains("private-login-password"));
        assert!(SystemCredentials::decode("123456").is_none());
        input.username = "root\0owner".into();
        assert!(!input.valid());
        input.username = "owner".into();
        input.password = "x".repeat(1025);
        assert!(!input.valid());
        input.password.clear();
        assert!(!input.valid());
        for (mode, index) in [
            (AuthenticationMode::PairingCode, 0),
            (AuthenticationMode::Password, 1),
            (AuthenticationMode::Otp, 2),
            (AuthenticationMode::None, 3),
            (AuthenticationMode::SystemAccount, 4),
        ] {
            assert_eq!(postcard::to_allocvec(&mode).unwrap(), vec![index]);
        }
    }
}

//! RVP/1 protocol constants (the single authoritative definition is .agents/protocol.md §2).

pub const PROTOCOL_NAME: &str = "RVP/1";
pub const PROTO_VERSION: u16 = 1;
pub const DEFAULT_PORT: u16 = 48688;
pub const MDNS_SERVICE: &str = "_removent._udp.local.";
pub const MAGIC: [u8; 4] = [0x52, 0x56, 0x50, 0x31];
pub const RESUME_WINDOW_SECS: u64 = 30;

pub const STREAM_TYPE_VIDEO: u8 = 0x01;
pub const STREAM_TYPE_AUDIO: u8 = 0x02;

pub const VIDEO_HEADER_LEN: usize = 27;
pub const AUDIO_HEADER_LEN: usize = 14;

pub const CONTROL_LEN_PREFIX: usize = 4;

pub const MAX_CONTROL_MSG_LEN: u32 = 8 * 1024 * 1024;

pub mod video_flags {
    pub const KEYFRAME: u8 = 1 << 0;
    pub const CONFIG_CHANGED: u8 = 1 << 1;
    pub const END_OF_STREAM: u8 = 1 << 2;
}

pub mod audio_flags {
    pub const DTX: u8 = 1 << 0;
    pub const MIC_DIRECTION: u8 = 1 << 1;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum CodecId {
    H264 = 0x01,
    Hevc = 0x02,
}

impl CodecId {
    pub fn payload_tag(self) -> u8 {
        match self {
            CodecId::H264 => 0x01,
            CodecId::Hevc => 0x02,
        }
    }
    pub fn from_payload_tag(tag: u8) -> Option<Self> {
        match tag {
            0x01 => Some(CodecId::H264),
            0x02 => Some(CodecId::Hevc),
            _ => None,
        }
    }
}

pub mod feature_bits {
    pub const FILE_TRANSFER: u64 = 1 << 0;
    pub const TWO_WAY_AUDIO: u64 = 1 << 1;
    pub const HDR: u64 = 1 << 2;
    pub const RELATIVE_POINTER: u64 = 1 << 3;
    pub const RICH_CLIPBOARD: u64 = 1 << 4;
}

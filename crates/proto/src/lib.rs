//! removent-proto: RVP/1 protocol definitions. Pure-function library with no IO dependencies (see .agents/architecture.md §2).

pub mod constants;
pub mod frames;
pub mod msg;

pub use constants::*;
pub use frames::{
    AudioPacketHeader, ControlDecodeOutcome, FrameError, VideoFrameHeader, build_audio_packet,
    build_video_frame, decode_control, encode_control, parse_audio_header, parse_video_header,
    write_audio_header, write_video_header,
};
pub use msg::*;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VersionError {
    #[error("protocol version mismatch: local={local}, remote={remote}")]
    Mismatch { local: u16, remote: u16 },
}

/// Version negotiation: RVP treats an exact PROTO_VERSION match as compatible (protocol.md §8).
pub fn negotiate_proto_version(local: u16, remote: u16) -> Result<u16, VersionError> {
    if local == remote {
        Ok(local)
    } else {
        Err(VersionError::Mismatch { local, remote })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_negotiation() {
        assert_eq!(negotiate_proto_version(1, 1).unwrap(), 1);
        assert!(negotiate_proto_version(1, 2).is_err());
        assert!(negotiate_proto_version(2, 1).is_err());
    }

    #[test]
    fn constants_match_design_doc() {
        assert_eq!(MAGIC, *b"RVP1");
        assert_eq!(DEFAULT_PORT, 48688);
        assert_eq!(MDNS_SERVICE, "_removent._udp.local.");
        assert_eq!(VIDEO_HEADER_LEN, 27);
        assert_eq!(AUDIO_HEADER_LEN, 14);
    }
}

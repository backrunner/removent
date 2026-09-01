//! Media frame packing/parsing (protocol.md §6.1–6.2) and control-stream framing (§6.3).
//!
//! Pure functions, no IO: upper layers use these primitives to drive QUIC streams.

use crate::constants::*;
use crate::msg::ControlMsg;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("buffer too small: need {need} bytes, got {got}")]
    TooSmall { need: usize, got: usize },
    #[error("invalid stream type: {0:#x}")]
    InvalidStreamType(u8),
    #[error("invalid codec tag: {0:#x}")]
    InvalidCodec(u8),
    #[error("payload length {declared} exceeds available {available}")]
    PayloadTooLarge { declared: u32, available: usize },
    #[error("control message too large: {0} bytes")]
    ControlTooLarge(u32),
    #[error("postcard decode failed: {0}")]
    Decode(String),
}

/// Parsed video frame header (payload not included).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoFrameHeader {
    pub frame_id: u64,
    pub pts_us: i64,
    pub flags: u8,
    pub codec: CodecId,
    pub width: u16,
    pub height: u16,
    pub payload_len: u32,
}

impl VideoFrameHeader {
    pub fn is_keyframe(&self) -> bool {
        self.flags & video_flags::KEYFRAME != 0
    }
    pub fn config_changed(&self) -> bool {
        self.flags & video_flags::CONFIG_CHANGED != 0
    }
    pub fn end_of_stream(&self) -> bool {
        self.flags & video_flags::END_OF_STREAM != 0
    }
}

/// Write the header into `dst`; the `payload` should follow immediately (the caller can append zero-copy).
/// u16/u32 fields are little-endian per protocol.md §6.1; u64/i64 stay big-endian.
pub fn write_video_header(dst: &mut Vec<u8>, h: &VideoFrameHeader) {
    dst.push(STREAM_TYPE_VIDEO);
    dst.extend_from_slice(&h.frame_id.to_be_bytes());
    dst.extend_from_slice(&h.pts_us.to_be_bytes());
    dst.push(h.flags);
    dst.push(h.codec.payload_tag());
    dst.extend_from_slice(&h.width.to_le_bytes());
    dst.extend_from_slice(&h.height.to_le_bytes());
    dst.extend_from_slice(&h.payload_len.to_le_bytes());
}

pub fn build_video_frame(h: &VideoFrameHeader, payload: &[u8]) -> Vec<u8> {
    debug_assert_eq!(payload.len() as u32, h.payload_len);
    let mut out = Vec::with_capacity(VIDEO_HEADER_LEN + payload.len());
    write_video_header(&mut out, h);
    out.extend_from_slice(payload);
    out
}

/// Parse the header from a buffer; returns (header, bytes consumed). Does not verify payload completeness (streamed reading is the upper layer's job).
pub fn parse_video_header(buf: &[u8]) -> Result<(VideoFrameHeader, usize), FrameError> {
    if buf.len() < VIDEO_HEADER_LEN {
        return Err(FrameError::TooSmall {
            need: VIDEO_HEADER_LEN,
            got: buf.len(),
        });
    }
    if buf[0] != STREAM_TYPE_VIDEO {
        return Err(FrameError::InvalidStreamType(buf[0]));
    }
    let frame_id = u64::from_be_bytes(buf[1..9].try_into().unwrap());
    let pts_us = i64::from_be_bytes(buf[9..17].try_into().unwrap());
    let flags = buf[17];
    let codec = CodecId::from_payload_tag(buf[18]).ok_or(FrameError::InvalidCodec(buf[18]))?;
    let width = u16::from_le_bytes(buf[19..21].try_into().unwrap());
    let height = u16::from_le_bytes(buf[21..23].try_into().unwrap());
    let payload_len = u32::from_le_bytes(buf[23..27].try_into().unwrap());
    Ok((
        VideoFrameHeader {
            frame_id,
            pts_us,
            flags,
            codec,
            width,
            height,
            payload_len,
        },
        VIDEO_HEADER_LEN,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioPacketHeader {
    pub seq: u16,
    pub pts_us: i64,
    pub flags: u8,
    pub payload_len: u16,
}

impl AudioPacketHeader {
    pub fn is_dtx(&self) -> bool {
        self.flags & audio_flags::DTX != 0
    }
}

pub fn write_audio_header(dst: &mut Vec<u8>, h: &AudioPacketHeader) {
    dst.push(STREAM_TYPE_AUDIO);
    dst.extend_from_slice(&h.seq.to_le_bytes());
    dst.extend_from_slice(&h.pts_us.to_be_bytes());
    dst.push(h.flags);
    dst.extend_from_slice(&h.payload_len.to_le_bytes());
}

pub fn build_audio_packet(h: &AudioPacketHeader, payload: &[u8]) -> Vec<u8> {
    debug_assert_eq!(payload.len() as u16, h.payload_len);
    let mut out = Vec::with_capacity(AUDIO_HEADER_LEN + payload.len());
    write_audio_header(&mut out, h);
    out.extend_from_slice(payload);
    out
}

pub fn parse_audio_header(buf: &[u8]) -> Result<(AudioPacketHeader, usize), FrameError> {
    if buf.len() < AUDIO_HEADER_LEN {
        return Err(FrameError::TooSmall {
            need: AUDIO_HEADER_LEN,
            got: buf.len(),
        });
    }
    if buf[0] != STREAM_TYPE_AUDIO {
        return Err(FrameError::InvalidStreamType(buf[0]));
    }
    let seq = u16::from_le_bytes(buf[1..3].try_into().unwrap());
    let pts_us = i64::from_be_bytes(buf[3..11].try_into().unwrap());
    let flags = buf[11];
    let payload_len = u16::from_le_bytes(buf[12..14].try_into().unwrap());
    Ok((
        AudioPacketHeader {
            seq,
            pts_us,
            flags,
            payload_len,
        },
        AUDIO_HEADER_LEN,
    ))
}

/// Total number of ControlMsg variants (protocol discipline: append only at the enum tail, protocol.md §8).
pub const CONTROL_MSG_VARIANTS: u64 = 26;

/// Parse a postcard varint (LEB128). Returns (value, bytes consumed).
pub fn parse_varint(buf: &[u8]) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    let mut shift = 0u32;
    for (i, &b) in buf.iter().enumerate() {
        if shift >= 64 {
            return None;
        }
        value |= ((b & 0x7F) as u64) << shift;
        if b & 0x80 == 0 {
            return Some((value, i + 1));
        }
        shift += 7;
    }
    None
}

/// Control message framing: `[u32 BE len][postcard]`. len covers the postcard part,
/// so a receiver can skip unknown variants wholesale (protocol.md §8).
pub fn encode_control(msg: &ControlMsg) -> Result<Vec<u8>, FrameError> {
    let body = postcard::to_allocvec(msg).map_err(|e| FrameError::Decode(e.to_string()))?;
    let total = CONTROL_LEN_PREFIX + body.len();
    if total > MAX_CONTROL_MSG_LEN as usize {
        return Err(FrameError::ControlTooLarge(total as u32));
    }
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

#[derive(Debug, PartialEq)]
pub enum ControlDecodeOutcome {
    Msg(Box<ControlMsg>),
    /// Unknown variant or undecodable message, skipped via the length prefix; value = total message length (including prefix).
    Skipped(usize),
}

/// Parse one control message from buffered bytes. Returns the total bytes consumed (including the 4-byte prefix).
pub fn decode_control(buf: &[u8]) -> Result<(ControlDecodeOutcome, usize), FrameError> {
    if buf.len() < CONTROL_LEN_PREFIX {
        return Err(FrameError::TooSmall {
            need: CONTROL_LEN_PREFIX,
            got: buf.len(),
        });
    }
    let body_len = u32::from_be_bytes(buf[..4].try_into().unwrap()) as usize;
    let total = CONTROL_LEN_PREFIX + body_len;
    if body_len as u32 > MAX_CONTROL_MSG_LEN {
        return Err(FrameError::ControlTooLarge(body_len as u32));
    }
    if buf.len() < total {
        return Err(FrameError::PayloadTooLarge {
            declared: body_len as u32,
            available: buf.len() - 4,
        });
    }
    match postcard::from_bytes::<ControlMsg>(&buf[4..total]) {
        Ok(msg) => Ok((ControlDecodeOutcome::Msg(Box::new(msg)), total)),
        Err(_) => match parse_varint(&buf[4..]) {
            Some((idx, _)) if idx >= CONTROL_MSG_VARIANTS => {
                Ok((ControlDecodeOutcome::Skipped(total), total))
            }
            Some(_) => Err(FrameError::Decode("postcard deserialization failed".into())),
            None => Err(FrameError::Decode("truncated control body".into())),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_video() -> VideoFrameHeader {
        VideoFrameHeader {
            frame_id: 0x1122_3344_5566_7788,
            pts_us: -123_456,
            flags: video_flags::KEYFRAME | video_flags::CONFIG_CHANGED,
            codec: CodecId::Hevc,
            width: 2560,
            height: 1440,
            payload_len: 5,
        }
    }

    #[test]
    fn video_header_roundtrip() {
        let h = sample_video();
        let buf = build_video_frame(&h, b"HELLO");
        assert_eq!(buf.len(), VIDEO_HEADER_LEN + 5);
        // u16/u32 fields are little-endian per protocol.md §6.1.
        assert_eq!(&buf[19..21], &2560u16.to_le_bytes());
        assert_eq!(&buf[21..23], &1440u16.to_le_bytes());
        assert_eq!(&buf[23..27], &5u32.to_le_bytes());
        let (parsed, consumed) = parse_video_header(&buf).unwrap();
        assert_eq!(consumed, VIDEO_HEADER_LEN);
        assert_eq!(parsed, h);
        assert!(parsed.is_keyframe() && parsed.config_changed());
    }

    #[test]
    fn video_header_rejects_bad_type_and_codec() {
        let mut buf = build_video_frame(&sample_video(), b"HELLO");
        buf[0] = 0x02;
        assert!(matches!(
            parse_video_header(&buf),
            Err(FrameError::InvalidStreamType(_))
        ));
        let mut buf2 = build_video_frame(&sample_video(), b"HELLO");
        buf2[18] = 0x7f;
        assert!(matches!(
            parse_video_header(&buf2),
            Err(FrameError::InvalidCodec(_))
        ));
    }

    #[test]
    fn audio_header_roundtrip() {
        let h = AudioPacketHeader {
            seq: 0xBEEF,
            pts_us: 999_999,
            flags: audio_flags::DTX,
            payload_len: 3,
        };
        let buf = build_audio_packet(&h, &[1, 2, 3]);
        assert_eq!(buf.len(), AUDIO_HEADER_LEN + 3);
        // u16 fields are little-endian per protocol.md §6.2.
        assert_eq!(&buf[1..3], &0xBEEFu16.to_le_bytes());
        assert_eq!(&buf[12..14], &3u16.to_le_bytes());
        let (parsed, consumed) = parse_audio_header(&buf).unwrap();
        assert_eq!(consumed, AUDIO_HEADER_LEN);
        assert!(matches!(parsed, ref p if *p == h));
        assert!(parsed.is_dtx());
    }

    #[test]
    fn control_encode_decode_roundtrip() {
        use crate::msg::*;
        let msgs = vec![
            ControlMsg::Ping { ts_us: 42 },
            ControlMsg::SessionRequest { caps: Caps::all() },
            ControlMsg::MouseEvent {
                display_id: 1,
                x_px: 123.5,
                y_px: 0.25,
                buttons: 1,
                kind: MouseKind::LeftDragged,
            },
            ControlMsg::KeyEvent {
                vk_code: 0x7D,
                modifiers: KeyModifiers::COMMAND | KeyModifiers::SHIFT,
                kind: KeyKind::Down,
                unicode: Some('é'),
            },
            ControlMsg::StatsReport {
                rtt_ms: 1.5,
                loss_pct: 0.0,
                recv_kbps: 12_000,
                jitter_ms: 0.3,
                decode_ms: 4.2,
                render_fps: 60.0,
            },
            ControlMsg::FileOffer {
                transfer_id: 7,
                name: "report.pdf".into(),
                size: 1024,
                sha256: [9u8; 32],
                to_remote: true,
            },
        ];
        for m in msgs {
            let wire = encode_control(&m).unwrap();
            let (out, consumed) = decode_control(&wire).unwrap();
            assert_eq!(consumed, wire.len());
            match out {
                ControlDecodeOutcome::Msg(got) => assert_eq!(*got, m),
                other => panic!("expected msg, got {other:?}"),
            }
        }
    }

    #[test]
    fn control_incomplete_returns_error() {
        let wire = encode_control(&ControlMsg::KeyframeRequest).unwrap();
        assert!(matches!(
            decode_control(&wire[..3]),
            Err(FrameError::TooSmall { .. })
        ));
        let (out, _) = decode_control(&wire).unwrap();
        assert!(matches!(out, ControlDecodeOutcome::Msg(_)));
    }

    #[test]
    fn unknown_variant_is_skipped_by_length_prefix() {
        let wire = encode_control(&ControlMsg::Ping { ts_us: 1 }).unwrap();
        let mut tampered = wire.clone();
        tampered[4] = 200;
        let (out, consumed) = decode_control(&tampered).unwrap();
        assert_eq!(consumed, wire.len());
        assert!(matches!(out, ControlDecodeOutcome::Skipped(n) if n == wire.len()));
    }

    #[test]
    fn control_variant_ordinals_are_stable() {
        use crate::msg::*;
        use ControlMsg as M;
        let probes: Vec<(ControlMsg, u64)> = vec![
            (M::SessionRequest { caps: Caps::none() }, 0),
            (M::SessionAccept, 1),
            (
                M::SessionReject {
                    reason: RejectReason::Busy,
                },
                2,
            ),
            (
                M::SessionEnd {
                    reason: EndReason::Shutdown,
                },
                3,
            ),
            (M::Ping { ts_us: 0 }, 4),
            (M::Pong { ts_us: 0 }, 5),
            (M::DisplayListUpdate { displays: vec![] }, 6),
            (M::SelectDisplay { id: 0 }, 7),
            (
                M::QualityControl {
                    bitrate_kbps: 0,
                    fps: 0,
                    scale: 1.0,
                },
                8,
            ),
            (M::KeyframeRequest, 9),
            (
                M::StatsReport {
                    rtt_ms: 0.0,
                    loss_pct: 0.0,
                    recv_kbps: 0,
                    jitter_ms: 0.0,
                    decode_ms: 0.0,
                    render_fps: 0.0,
                },
                10,
            ),
            (
                M::MouseEvent {
                    display_id: 0,
                    x_px: 0.0,
                    y_px: 0.0,
                    buttons: 0,
                    kind: MouseKind::Moved,
                },
                11,
            ),
            (
                M::ScrollEvent {
                    display_id: 0,
                    dx_mm: 0.0,
                    dy_mm: 0.0,
                    phase: ScrollPhase::Began,
                },
                12,
            ),
            (
                M::KeyEvent {
                    vk_code: 0,
                    modifiers: KeyModifiers::empty(),
                    kind: KeyKind::Down,
                    unicode: None,
                },
                13,
            ),
            (
                M::CursorShape {
                    cursor_id: 0,
                    w: 0,
                    h: 0,
                    hotspot_x: 0,
                    hotspot_y: 0,
                    rgba: vec![],
                },
                14,
            ),
            (
                M::CursorPosition {
                    x_px: 0.0,
                    y_px: 0.0,
                    visible: false,
                },
                15,
            ),
            (
                M::ClipboardSync {
                    seq: 0,
                    format: ClipFormat::TextUtf8,
                    data: vec![],
                },
                16,
            ),
            (M::ClipboardAck { seq: 0 }, 17),
            (
                M::FileOffer {
                    transfer_id: 0,
                    name: String::new(),
                    size: 0,
                    sha256: [0; 32],
                    to_remote: false,
                },
                18,
            ),
            (
                M::FileAccept {
                    transfer_id: 0,
                    offset: 0,
                },
                19,
            ),
            (
                M::FileProgress {
                    transfer_id: 0,
                    bytes_done: 0,
                },
                20,
            ),
            (
                M::FileComplete {
                    transfer_id: 0,
                    sha256_ok: true,
                },
                21,
            ),
            (
                M::FileCancel {
                    transfer_id: 0,
                    reason: String::new(),
                },
                22,
            ),
            (
                M::Error {
                    code: ErrorCode::Internal,
                },
                23,
            ),
        ];
        for (msg, expected) in probes {
            let wire = encode_control(&msg).unwrap();
            let (idx, _) = parse_varint(&wire[4..]).unwrap();
            assert_eq!(idx, expected, "variant ordinal drifted for {expected}");
        }
    }

    #[test]
    fn oversized_control_rejected() {
        assert!(matches!(
            decode_control(&[0xFF, 0xFF, 0xFF, 0xFF]),
            Err(FrameError::ControlTooLarge(_))
        ));
    }
}

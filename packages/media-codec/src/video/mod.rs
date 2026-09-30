//! Video codec: VideoToolbox hardware encode/decode wrappers.
//!
//! H264/HEVC frame data is Annex-B (with start codes); AV1 frame data is a
//! complete temporal-unit OBU stream. Parameter sets are inlined before
//! H264/HEVC keyframes (H264: SPS+PPS, HEVC: VPS+SPS+PPS), while rav1e emits
//! the AV1 sequence header in keyframe temporal units.
//! The decoder extracts parameter sets from Annex-B to create the
//! CMVideoFormatDescription.

use crate::av1::{Av1Decoder, Av1Encoder};
use crate::cm_ffi as cm;
use crate::compat_decompression::CompatibleDecompressionSession;
use apple_cf::iosurface::IOSurface;
use std::os::raw::c_void;
use std::sync::{Arc, mpsc};
use videotoolbox::session::Codec;

pub const BGRA_FOURCC: u32 = u32::from_be_bytes(*b"BGRA");
/// CoreMedia four-character code for AV1 (`av01`). The software AV1 path is
/// used for media today; this constant remains useful for hardware probes.
pub const AV1_CODEC_TYPE: u32 = u32::from_be_bytes(*b"av01");
const TIMESCALE_US: i32 = 1_000_000;
/// Limit quantization damage to screen text. A codec guardrail, not a
/// perceptual guarantee for every font, display scale or chroma pattern.
pub const SCREEN_MAX_FRAME_QP: i32 = 28;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Av1HardwareSupport {
    /// Whether VideoToolbox reports a hardware AV1 decoder on this Mac.
    pub decoder: bool,
    /// Whether the installed VideoToolbox encoder list contains a hardware
    /// AV1 encoder for the current OS/device.
    pub encoder: bool,
}

/// Probe VideoToolbox's AV1 hardware support without creating a session or
/// changing protocol negotiation. Software AV1 support is provided by rav1e
/// and rav1d independently of this probe.
pub fn av1_hardware_support() -> Av1HardwareSupport {
    let decoder = unsafe { videotoolbox::ffi::VTIsHardwareDecodeSupported(AV1_CODEC_TYPE) != 0 };
    let encoder = videotoolbox::available_video_encoder_details()
        .ok()
        .into_iter()
        .flatten()
        .any(|entry| {
            entry.base.codec_type == AV1_CODEC_TYPE && entry.is_hardware_accelerated == Some(true)
        });
    Av1HardwareSupport { decoder, encoder }
}

fn is_hevc(codec: removent_proto::CodecId) -> bool {
    matches!(codec, removent_proto::CodecId::Hevc)
}

#[derive(Debug, thiserror::Error)]
pub enum VideoError {
    #[error("videotoolbox: {0}")]
    Vt(#[from] videotoolbox::VTError),
    #[error("surface: {0}")]
    Surface(String),
    #[error("pixel data size mismatch: need {need}, got {got}")]
    PixelSizeMismatch { need: usize, got: usize },
    #[error("parameter sets unavailable before first encode")]
    NoParameterSets,
    #[error("core media status {0}")]
    CoreMedia(i32),
    #[error("decode failed with status {0}")]
    DecodeStatus(i32),
    #[error("core media returned null sample buffer")]
    NullSampleBuffer,
    #[error("avcc length prefix invalid")]
    InvalidAvcc,
    #[error("software AV1: {0}")]
    Av1(String),
}

/// One encoded frame (Annex-B for H264/HEVC, temporal-unit OBUs for AV1).
#[derive(Debug, Clone)]
pub struct EncodedVideoFrame {
    pub data: Vec<u8>,
    pub pts_us: i64,
    pub keyframe: bool,
}

fn vt_codec(codec: removent_proto::CodecId) -> Codec {
    match codec {
        removent_proto::CodecId::H264 => Codec::H264,
        removent_proto::CodecId::Hevc => Codec::HEVC,
        removent_proto::CodecId::Av1 => unreachable!("AV1 uses the software codec path"),
    }
}

mod annexb;
mod decoder;
mod encoder;
mod pixels;
#[cfg(test)]
mod tests;

pub use annexb::annexb_has_idr;
pub use annexb::extract_param_sets;
use annexb::{START_CODE, is_param_set, nal_type};
pub use annexb::{avcc_to_annexb, split_annexb_nals};
pub use decoder::DecodedBgra;
pub use decoder::VideoDecoder;
use decoder::{extract_param_sets_from_desc, format_description_of_sample, is_sync_sample};
pub use encoder::VideoEncoder;
#[cfg(test)]
use encoder::encode_maybe_forced;
use pixels::pixel_buffer_to_bgra;
#[cfg(test)]
use pixels::{YuvColor, nv12_to_bgra, p010_video_to_bgra};

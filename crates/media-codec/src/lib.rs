//! removent-media-codec: VideoToolbox/AV1 video codecs and Opus audio codec.

pub mod audio;
mod av1;
pub mod cm_ffi;
mod compat_decompression;
pub mod video;

pub use audio::{Application, AudioDecoder, AudioEncoder, AudioError, FRAME_SAMPLES_PER_CHANNEL};
pub use video::{
    AV1_CODEC_TYPE, Av1HardwareSupport, DecodedBgra, EncodedVideoFrame, VideoDecoder, VideoEncoder,
    VideoError, annexb_has_idr, av1_hardware_support, avcc_to_annexb, extract_param_sets,
    split_annexb_nals,
};

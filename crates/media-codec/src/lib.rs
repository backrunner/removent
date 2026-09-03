//! removent-media-codec: VideoToolbox video codec and Opus audio codec.

pub mod audio;
pub mod cm_ffi;
pub mod video;

pub use audio::{Application, AudioDecoder, AudioEncoder, AudioError, FRAME_SAMPLES_PER_CHANNEL};
pub use video::{
    AV1_CODEC_TYPE, Av1HardwareSupport, DecodedBgra, EncodedVideoFrame, VideoDecoder, VideoEncoder,
    VideoError, annexb_has_idr, av1_hardware_support, avcc_to_annexb, extract_param_sets,
    split_annexb_nals,
};

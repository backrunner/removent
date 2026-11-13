//! removent-media-codec: VideoToolbox/AV1 video codecs and Opus audio codec.

pub mod audio;
mod av1;
#[cfg(target_vendor = "apple")]
pub mod cm_ffi;
#[cfg(target_vendor = "apple")]
mod compat_decompression;
pub mod scale;
#[cfg(target_vendor = "apple")]
pub mod video;
#[cfg(not(target_vendor = "apple"))]
#[path = "portable.rs"]
pub mod video;

pub use audio::{Application, AudioDecoder, AudioEncoder, AudioError, FRAME_SAMPLES_PER_CHANNEL};
pub use video::{
    AV1_CODEC_TYPE, Av1HardwareSupport, DecodedBgra, EncodedVideoFrame, VideoDecoder, VideoEncoder,
    VideoError, annexb_has_idr, av1_hardware_support, avcc_to_annexb, extract_param_sets,
    split_annexb_nals,
};

#[cfg(target_vendor = "apple")]
pub use apple_cf::cv::CVPixelBuffer as NativePixelBuffer;
#[cfg(not(target_vendor = "apple"))]
#[derive(Debug, Clone)]
pub enum NativePixelBuffer {}

//! CPU H.264/AV1 implementation for Windows and Linux.
use crate::av1::{Av1Decoder, Av1Encoder};
use removent_proto::CodecId;
use std::sync::{Arc, Mutex, mpsc};
#[path = "video/annexb.rs"]
mod annexb;
pub use annexb::{annexb_has_idr, avcc_to_annexb, extract_param_sets, split_annexb_nals};
pub const AV1_CODEC_TYPE: u32 = u32::from_be_bytes(*b"av01");
#[derive(Debug, Clone, Copy)]
pub struct Av1HardwareSupport {
    pub decoder: bool,
    pub encoder: bool,
}
pub fn av1_hardware_support() -> Av1HardwareSupport {
    Av1HardwareSupport {
        decoder: false,
        encoder: false,
    }
}
#[derive(Debug, thiserror::Error)]
pub enum VideoError {
    #[error("H264: {0}")]
    H264(#[from] openh264::Error),
    #[error("unsupported codec")]
    Unsupported,
    #[error("GPU processing: {0}")]
    Gpu(String),
    #[error("surface: {0}")]
    Surface(String),
    #[error("pixel data size mismatch: need {need}, got {got}")]
    PixelSizeMismatch { need: usize, got: usize },
    #[error("video geometry mismatch: expected {expected:?}, got {actual:?}")]
    GeometryMismatch {
        expected: (usize, usize),
        actual: (i32, i32),
    },
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

pub struct DecodedBgra {
    pub surface: Option<crate::NativePixelBuffer>,
    pub data: Vec<u8>,
    pub pts_us: i64,
}
pub struct VideoEncoder {
    h264: Option<openh264::encoder::Encoder>,
    av1: Option<Av1Encoder>,
    width: usize,
    height: usize,
    fps: u8,
    bitrate: u32,
    params: Option<Vec<Vec<u8>>>,
}
impl VideoEncoder {
    pub fn new(
        codec: CodecId,
        width: usize,
        height: usize,
        bitrate: u32,
        fps: u8,
    ) -> Result<Self, VideoError> {
        if width == 0 || height == 0 || fps == 0 || width % 2 != 0 || height % 2 != 0 {
            return Err(VideoError::Surface("invalid video geometry".into()));
        }
        let (h264, av1) = match codec {
            CodecId::H264 => (Some(Self::h264(bitrate, fps)?), None),
            CodecId::Av1 => (None, Some(Av1Encoder::new(width, height, bitrate, fps)?)),
            _ => return Err(VideoError::Unsupported),
        };
        Ok(Self {
            h264,
            av1,
            width,
            height,
            fps,
            bitrate,
            params: None,
        })
    }
    fn h264(bitrate: u32, fps: u8) -> Result<openh264::encoder::Encoder, VideoError> {
        use openh264::encoder::*;
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(bitrate.saturating_mul(1000)))
            .max_frame_rate(FrameRate::from_hz(fps as f32));
        Ok(Encoder::with_api_config(
            openh264::OpenH264API::from_source(),
            config,
        )?)
    }
    pub fn hardware_accelerated(&self) -> bool {
        false
    }
    pub fn quality_ceiling_supported(&self) -> bool {
        self.av1.is_some()
    }
    pub fn width(&self) -> usize {
        self.width
    }
    pub fn height(&self) -> usize {
        self.height
    }
    pub fn parameter_sets(&self) -> Option<&[Vec<u8>]> {
        self.params.as_deref()
    }
    pub fn request_keyframe(&mut self) {
        if let Some(e) = &mut self.h264 {
            e.force_intra_frame();
        }
        if let Some(e) = &mut self.av1 {
            e.request_keyframe();
        }
    }
    pub fn set_bitrate_kbps(&mut self, bitrate: u32) -> Result<(), VideoError> {
        if let Some(e) = &mut self.av1 {
            return e.set_bitrate_kbps(bitrate);
        }
        if bitrate != self.bitrate {
            self.h264 = Some(Self::h264(bitrate, self.fps)?);
            self.bitrate = bitrate;
        }
        Ok(())
    }
    pub fn flush(&mut self) -> Result<Vec<EncodedVideoFrame>, VideoError> {
        self.av1.as_mut().map_or(Ok(vec![]), |e| e.flush())
    }
    pub fn encode_bgra(
        &mut self,
        bgra: &[u8],
        pts_us: i64,
    ) -> Result<Vec<EncodedVideoFrame>, VideoError> {
        let need = self.width * self.height * 4;
        if bgra.len() != need {
            return Err(VideoError::PixelSizeMismatch {
                need,
                got: bgra.len(),
            });
        }
        if let Some(e) = &mut self.av1 {
            return e.encode_bgra(bgra, pts_us);
        }
        let rgb: Vec<u8> = bgra
            .chunks_exact(4)
            .flat_map(|p| [p[2], p[1], p[0]])
            .collect();
        let yuv = openh264::formats::YUVBuffer::from_rgb_source(
            openh264::formats::RgbSliceU8::new(&rgb, (self.width, self.height)),
        );
        let data = self.h264.as_mut().unwrap().encode(&yuv)?.to_vec();
        if data.is_empty() {
            return Ok(vec![]);
        }
        let keyframe = annexb_has_idr(&data, false);
        if keyframe {
            self.params = Some(extract_param_sets(&data, false));
        }
        Ok(vec![EncodedVideoFrame {
            data,
            pts_us,
            keyframe,
        }])
    }
    pub fn encode_native(
        &mut self,
        buffer: &crate::NativePixelBuffer,
        _pts: i64,
    ) -> Result<Vec<EncodedVideoFrame>, VideoError> {
        match *buffer {}
    }
}
enum Decoder {
    H264(openh264::decoder::Decoder),
    Av1(Av1Decoder),
}
pub struct VideoDecoder {
    decoder: Mutex<Decoder>,
    width: usize,
    height: usize,
    output: Arc<dyn Fn(DecodedBgra) + Send + Sync>,
    rx: Mutex<mpsc::Receiver<DecodedBgra>>,
}
impl VideoDecoder {
    pub fn new(
        codec: CodecId,
        width: usize,
        height: usize,
        params: &[Vec<u8>],
    ) -> Result<Self, VideoError> {
        let (tx, rx) = mpsc::channel();
        let mut d = Self::with_output(codec, width, height, params, move |f| {
            let _ = tx.send(f);
        })?;
        d.rx = Mutex::new(rx);
        Ok(d)
    }
    pub fn with_output(
        codec: CodecId,
        width: usize,
        height: usize,
        params: &[Vec<u8>],
        output: impl Fn(DecodedBgra) + Send + Sync + 'static,
    ) -> Result<Self, VideoError> {
        Self::with_surface_output(codec, width, height, params, false, output)
    }
    pub fn with_surface_output(
        codec: CodecId,
        width: usize,
        height: usize,
        params: &[Vec<u8>],
        _native: bool,
        output: impl Fn(DecodedBgra) + Send + Sync + 'static,
    ) -> Result<Self, VideoError> {
        let decoder = match codec {
            CodecId::H264 => {
                let mut d = openh264::decoder::Decoder::new()?;
                for p in params {
                    let mut bytes = vec![0, 0, 0, 1];
                    bytes.extend(p);
                    d.decode(&bytes)?;
                }
                Decoder::H264(d)
            }
            CodecId::Av1 => Decoder::Av1(Av1Decoder::new(width, height)?),
            _ => return Err(VideoError::Unsupported),
        };
        Ok(Self {
            decoder: Mutex::new(decoder),
            width,
            height,
            output: Arc::new(output),
            rx: Mutex::new(mpsc::channel().1),
        })
    }
    pub fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }
    pub fn try_recv_decoded(&self) -> Option<DecodedBgra> {
        self.rx.lock().unwrap().try_recv().ok()
    }
    pub fn decode_annexb(&self, bytes: &[u8], pts_us: i64) -> Result<(), VideoError> {
        use openh264::formats::YUVSource;
        match &mut *self.decoder.lock().unwrap() {
            Decoder::Av1(d) => {
                for frame in d.decode(bytes, pts_us)? {
                    (self.output)(frame);
                }
            }
            Decoder::H264(d) => {
                if let Some(yuv) = d.decode(bytes)? {
                    if yuv.dimensions() != (self.width, self.height) {
                        return Err(VideoError::Surface(
                            "decoded dimensions differ from negotiated frame".into(),
                        ));
                    }
                    let mut rgba = vec![0; self.width * self.height * 4];
                    yuv.write_rgba8(&mut rgba);
                    for p in rgba.chunks_exact_mut(4) {
                        p.swap(0, 2);
                    }
                    (self.output)(DecodedBgra {
                        surface: None,
                        data: rgba,
                        pts_us,
                    });
                }
            }
        }
        Ok(())
    }
    pub fn flush(&self) -> Result<(), VideoError> {
        if let Decoder::Av1(d) = &mut *self.decoder.lock().unwrap() {
            for frame in d.flush()? {
                (self.output)(frame);
            }
        }
        Ok(())
    }
}

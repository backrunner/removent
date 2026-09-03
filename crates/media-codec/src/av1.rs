//! Software AV1 codec path used when VideoToolbox AV1 is unavailable.
//!
//! The wire payload is the complete low-latency rav1e temporal unit (OBUs). A
//! key temporal unit includes its sequence header, so the decoder does not
//! need a separate `av1C` configuration blob.

use rav1e::prelude::*;
use rusty_av1d::{Decoder, PlanarImageComponent, Rav1dError};
use std::collections::VecDeque;

use crate::video::{DecodedBgra, EncodedVideoFrame, VideoError};

pub(crate) struct Av1Encoder {
    ctx: Context<u8>,
    width: usize,
    height: usize,
    force_keyframe: bool,
    pts: VecDeque<i64>,
    pts_base: u64,
    fps: u8,
    bitrate_kbps: u32,
}

impl Av1Encoder {
    pub(crate) fn new(
        width: usize,
        height: usize,
        bitrate_kbps: u32,
        fps: u8,
    ) -> Result<Self, VideoError> {
        if width < 16 || height < 16 {
            return Err(VideoError::Av1(format!(
                "dimensions {}x{} are below rav1e's 16-pixel minimum",
                width, height
            )));
        }
        if fps == 0 {
            return Err(VideoError::Av1("frame rate must be non-zero".into()));
        }
        let mut enc = EncoderConfig::with_speed_preset(10);
        enc.width = width;
        enc.height = height;
        enc.chroma_sampling = ChromaSampling::Cs420;
        enc.pixel_range = PixelRange::Limited;
        enc.time_base = Rational::new(1, u64::from(fps));
        enc.low_latency = true;
        enc.speed_settings.rdo_lookahead_frames = 1;
        enc.bitrate = (bitrate_kbps.saturating_mul(1_000)).min(i32::MAX as u32) as i32;
        enc.min_key_frame_interval = u64::from(fps.max(1));
        enc.max_key_frame_interval = u64::from(fps.max(1)).saturating_mul(2);
        let threads = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(8);
        let cfg = Config::new().with_encoder_config(enc).with_threads(threads);
        let ctx = cfg
            .new_context()
            .map_err(|e| VideoError::Av1(format!("rav1e config: {e}")))?;
        Ok(Self {
            ctx,
            width,
            height,
            force_keyframe: false,
            pts: VecDeque::new(),
            pts_base: 0,
            fps,
            bitrate_kbps,
        })
    }

    pub(crate) fn request_keyframe(&mut self) {
        self.force_keyframe = true;
    }

    pub(crate) fn set_bitrate_kbps(&mut self, bitrate_kbps: u32) -> Result<(), VideoError> {
        if bitrate_kbps == self.bitrate_kbps {
            return Ok(());
        }
        let mut next = Self::new(self.width, self.height, bitrate_kbps, self.fps)?;
        next.force_keyframe = true;
        *self = next;
        Ok(())
    }

    pub(crate) fn encode_bgra(
        &mut self,
        bgra: &[u8],
        pts_us: i64,
    ) -> Result<Vec<EncodedVideoFrame>, VideoError> {
        let need = self.width.saturating_mul(self.height).saturating_mul(4);
        if bgra.len() != need {
            return Err(VideoError::PixelSizeMismatch {
                need,
                got: bgra.len(),
            });
        }
        let mut frame = self.ctx.new_frame();
        let cw = self.width.div_ceil(2);
        let ch = self.height.div_ceil(2);
        let mut y = vec![0u8; self.width * self.height];
        let mut u = vec![0u8; cw * ch];
        let mut v = vec![0u8; cw * ch];
        bgra_to_yuv420(bgra, self.width, self.height, &mut y, &mut u, &mut v);
        frame.planes[0].copy_from_raw_u8(&y, self.width, 1);
        frame.planes[1].copy_from_raw_u8(&u, cw, 1);
        frame.planes[2].copy_from_raw_u8(&v, cw, 1);
        frame.planes[0].pad(self.width, self.height);
        frame.planes[1].pad(self.width, self.height);
        frame.planes[2].pad(self.width, self.height);
        let params = FrameParameters {
            frame_type_override: if std::mem::take(&mut self.force_keyframe) {
                FrameTypeOverride::Key
            } else {
                FrameTypeOverride::No
            },
            ..Default::default()
        };
        self.pts.push_back(pts_us);
        self.ctx
            .send_frame((frame, params))
            .map_err(|e| VideoError::Av1(format!("rav1e send: {e}")))?;

        self.drain_packets(pts_us)
    }

    pub(crate) fn flush(&mut self) -> Result<Vec<EncodedVideoFrame>, VideoError> {
        self.ctx.flush();
        self.drain_packets(self.pts.back().copied().unwrap_or_default())
    }

    fn drain_packets(
        &mut self,
        fallback_pts_us: i64,
    ) -> Result<Vec<EncodedVideoFrame>, VideoError> {
        let mut out = Vec::new();
        loop {
            match self.ctx.receive_packet() {
                Ok(packet) => {
                    let packet_index = packet.input_frameno;
                    let pts = if packet_index >= self.pts_base {
                        self.pts
                            .get((packet_index - self.pts_base) as usize)
                            .copied()
                            .unwrap_or(fallback_pts_us)
                    } else {
                        fallback_pts_us
                    };
                    // Packets are emitted in input order in low-latency mode.
                    // Retain only timestamps that can still be referenced by a
                    // delayed packet.
                    while self.pts_base < packet_index {
                        self.pts.pop_front();
                        self.pts_base += 1;
                    }
                    if self.pts_base == packet_index {
                        self.pts.pop_front();
                        self.pts_base += 1;
                    }
                    out.push(EncodedVideoFrame {
                        data: packet.data,
                        pts_us: pts,
                        keyframe: packet.frame_type == FrameType::KEY,
                    })
                }
                Err(EncoderStatus::NeedMoreData | EncoderStatus::Encoded) => break,
                Err(EncoderStatus::LimitReached) => break,
                Err(e) => return Err(VideoError::Av1(format!("rav1e receive: {e}"))),
            }
        }
        Ok(out)
    }
}

pub(crate) struct Av1Decoder {
    decoder: Decoder,
    width: usize,
    height: usize,
}

impl Av1Decoder {
    pub(crate) fn new(width: usize, height: usize) -> Result<Self, VideoError> {
        let mut settings = rusty_av1d::Settings::new();
        settings.set_n_threads(
            std::thread::available_parallelism()
                .map_or(1, usize::from)
                .min(8) as u32,
        );
        settings.set_max_frame_delay(1);
        let decoder = Decoder::with_settings(&settings)
            .map_err(|e| VideoError::Av1(format!("rav1d init: {e}")))?;
        Ok(Self {
            decoder,
            width,
            height,
        })
    }

    pub(crate) fn decode(
        &mut self,
        payload: &[u8],
        pts_us: i64,
    ) -> Result<Vec<DecodedBgra>, VideoError> {
        let mut send = self.decoder.send_data(
            payload.to_vec().into_boxed_slice(),
            None,
            Some(pts_us),
            None,
        );
        loop {
            let mut pictures = self.drain_pictures(pts_us)?;
            match send {
                Ok(()) => {
                    pictures.extend(self.drain_pictures(pts_us)?);
                    return Ok(pictures);
                }
                Err(Rav1dError::TryAgain) => send = self.decoder.send_pending_data(),
                Err(e) => return Err(VideoError::Av1(format!("rav1d send: {e}"))),
            }
        }
    }

    pub(crate) fn flush(&mut self) -> Result<Vec<DecodedBgra>, VideoError> {
        self.decoder.flush();
        self.drain_pictures(0)
    }

    fn drain_pictures(&mut self, fallback_pts_us: i64) -> Result<Vec<DecodedBgra>, VideoError> {
        let mut out = Vec::new();
        loop {
            match self.decoder.get_picture() {
                Ok(pic) => {
                    let width = pic.width() as usize;
                    let height = pic.height() as usize;
                    if width != self.width || height != self.height {
                        return Err(VideoError::Av1(format!(
                            "decoded dimensions {}x{} differ from {}x{}",
                            width, height, self.width, self.height
                        )));
                    }
                    out.push(DecodedBgra {
                        data: yuv420_to_bgra(&pic, width, height),
                        pts_us: pic.timestamp().unwrap_or(fallback_pts_us),
                    });
                }
                Err(Rav1dError::TryAgain) => break,
                Err(e) => return Err(VideoError::Av1(format!("rav1d picture: {e}"))),
            }
        }
        Ok(out)
    }
}

fn clamp(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

fn bgra_to_yuv420(
    bgra: &[u8],
    width: usize,
    height: usize,
    y: &mut [u8],
    u: &mut [u8],
    v: &mut [u8],
) {
    for py in 0..height {
        for px in 0..width {
            let i = (py * width + px) * 4;
            let b = i32::from(bgra[i]);
            let g = i32::from(bgra[i + 1]);
            let r = i32::from(bgra[i + 2]);
            y[py * width + px] = clamp(((47 * r + 157 * g + 16 * b + 128) >> 8) + 16);
        }
    }
    let cw = width.div_ceil(2);
    for py in 0..height.div_ceil(2) {
        for px in 0..cw {
            let mut sr = 0i32;
            let mut sg = 0i32;
            let mut sb = 0i32;
            let mut n = 0i32;
            for dy in 0..2 {
                for dx in 0..2 {
                    let x = px * 2 + dx;
                    let yy = py * 2 + dy;
                    if x < width && yy < height {
                        let i = (yy * width + x) * 4;
                        sb += i32::from(bgra[i]);
                        sg += i32::from(bgra[i + 1]);
                        sr += i32::from(bgra[i + 2]);
                        n += 1;
                    }
                }
            }
            let r = sr / n;
            let g = sg / n;
            let b = sb / n;
            u[py * cw + px] = clamp(((-26 * r - 87 * g + 112 * b + 128) >> 8) + 128);
            v[py * cw + px] = clamp(((112 * r - 102 * g - 10 * b + 128) >> 8) + 128);
        }
    }
}

fn yuv420_to_bgra(pic: &rusty_av1d::Picture, width: usize, height: usize) -> Vec<u8> {
    let y = pic.plane(PlanarImageComponent::Y);
    let u = pic.plane(PlanarImageComponent::U);
    let v = pic.plane(PlanarImageComponent::V);
    let ys = pic.stride(PlanarImageComponent::Y) as usize;
    let us = pic.stride(PlanarImageComponent::U) as usize;
    let vs = pic.stride(PlanarImageComponent::V) as usize;
    let mut out = vec![0u8; width * height * 4];
    for py in 0..height {
        for px in 0..width {
            let yy = i32::from(y[py * ys + px]) - 16;
            let uu = i32::from(u[(py / 2) * us + px / 2]) - 128;
            let vv = i32::from(v[(py / 2) * vs + px / 2]) - 128;
            let r = (298 * yy + 459 * vv + 128) >> 8;
            let g = (298 * yy - 55 * uu - 136 * vv + 128) >> 8;
            let b = (298 * yy + 540 * uu + 128) >> 8;
            let i = (py * width + px) * 4;
            out[i] = clamp(b);
            out[i + 1] = clamp(g);
            out[i + 2] = clamp(r);
            out[i + 3] = 255;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_yuv_handles_odd_dimensions() {
        let src = vec![128u8; 17 * 19 * 4];
        let mut y = vec![0; 17 * 19];
        let mut u = vec![0; 9 * 10];
        let mut v = vec![0; 9 * 10];
        bgra_to_yuv420(&src, 17, 19, &mut y, &mut u, &mut v);
        assert!(y.iter().all(|x| *x > 0));
        assert_eq!(u.len(), 90);
        assert_eq!(v.len(), 90);
    }

    #[test]
    fn software_roundtrip_emits_av1_obus() {
        let mut enc = Av1Encoder::new(32, 32, 500, 30).expect("encoder");
        let frame = vec![64u8; 32 * 32 * 4];
        let mut packets = enc.encode_bgra(&frame, 7).expect("encode");
        for pts in 8..16 {
            let got = enc.encode_bgra(&frame, pts).expect("encode frame");
            packets.extend(got);
        }
        assert!(!packets.is_empty());
        assert!(packets[0].keyframe);
        let mut dec = Av1Decoder::new(32, 32).expect("decoder");
        let decoded = dec.decode(&packets[0].data, 7).expect("decode");
        assert!(!decoded.is_empty());
        assert_eq!(decoded[0].data.len(), 32 * 32 * 4);
    }

    #[test]
    fn software_roundtrip_handles_odd_dimensions() {
        let (width, height) = (33, 31);
        let mut enc = Av1Encoder::new(width, height, 500, 30).expect("encoder");
        let frame = vec![96u8; width * height * 4];
        let mut packets = enc.encode_bgra(&frame, 11).expect("encode");
        packets.extend(enc.flush().expect("flush"));
        let first = packets.first().expect("encoded packet");
        let mut dec = Av1Decoder::new(width, height).expect("decoder");
        let mut decoded = dec.decode(&first.data, first.pts_us).expect("decode");
        decoded.extend(dec.flush().expect("decoder flush"));
        let picture = decoded.first().expect("decoded picture");
        assert_eq!(picture.data.len(), width * height * 4);
    }

    #[test]
    fn keyframe_request_is_honoured() {
        let mut enc = Av1Encoder::new(32, 32, 500, 30).expect("encoder");
        let frame = vec![64u8; 32 * 32 * 4];
        let mut packets = Vec::new();
        for pts in 0..8 {
            packets.extend(enc.encode_bgra(&frame, pts).expect("encode"));
        }
        assert!(packets.iter().any(|p| p.keyframe));
        enc.request_keyframe();
        for pts in 8..16 {
            packets.extend(enc.encode_bgra(&frame, pts).expect("encode"));
        }
        let forced = packets
            .iter()
            .skip(1)
            .find(|p| p.keyframe)
            .expect("forced keyframe");
        // A recovery decoder starts with no sequence-header state. Every rav1e
        // key temporal unit must therefore be independently decodable.
        let mut recovery = Av1Decoder::new(32, 32).expect("recovery decoder");
        let decoded = recovery
            .decode(&forced.data, forced.pts_us)
            .expect("decode forced keyframe");
        assert!(!decoded.is_empty());
    }
}

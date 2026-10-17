use super::*;

pub(super) fn video_limits(
    cfg: &HostConfig,
    display: &removent_proto::DisplayInfo,
    quality: Option<removent_proto::VideoQuality>,
) -> (u8, u32) {
    let fps = cfg
        .video_fps
        .min(if quality.is_some() {
            removent_input::display::capture_frame_rate(display.id as u32)
        } else {
            60
        })
        .max(1);
    let bitrate = cfg
        .video_bitrate_kbps
        .min(quality.map_or(24_000, |q| q.bitrate_kbps(display.w_px, display.h_px, fps)))
        .max(1);
    (fps, bitrate)
}

/// AV1 is intentionally opt-in for remote desktop sessions. Software AV1 has
/// materially higher CPU cost and a few frames of encoder pipeline delay, so a
/// peer must advertise the decoder feature and the operator must request it via
/// `REMOVENT_VIDEO_CODEC=av1`. Any failed probe falls back to HEVC.
pub(super) fn preferred_video_codec(
    peer_features: u64,
    display_w: u32,
    display_h: u32,
    bitrate_kbps: u32,
    fps: u8,
) -> CodecId {
    let requested = std::env::var("REMOVENT_VIDEO_CODEC")
        .ok()
        .is_some_and(|v| v.eq_ignore_ascii_case("av1") || v.eq_ignore_ascii_case("software-av1"));
    if !requested || peer_features & removent_proto::feature_bits::SOFTWARE_AV1 == 0 {
        return CodecId::Hevc;
    }
    let (w, h) = fit_capture_dims(display_w, display_h);
    if removent_media_codec::VideoEncoder::new(
        CodecId::Av1,
        w as usize,
        h as usize,
        bitrate_kbps,
        fps,
    )
    .is_ok()
    {
        CodecId::Av1
    } else {
        tracing::warn!("software AV1 probe failed; falling back to HEVC");
        CodecId::Hevc
    }
}

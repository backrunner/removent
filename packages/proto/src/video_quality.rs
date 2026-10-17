//! Per-connection dynamic resolution ceilings. Portrait sources use the same
//! long/short-edge bounds; source pixels are never upscaled.
use serde::{Deserialize, Serialize};

pub const MAX_VIDEO_FPS: u8 = 240;
pub const MAX_VIDEO_BITRATE_KBPS: u32 = 256_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoQuality {
    Low,
    #[default]
    Standard,
    High,
    Ultra,
}

impl VideoQuality {
    pub const ALL: [Self; 4] = [Self::Low, Self::Standard, Self::High, Self::Ultra];

    pub fn min_height(self) -> u32 {
        if self == Self::Low { 480 } else { 540 }
    }

    pub fn max_dimensions(self) -> (u32, u32) {
        match self {
            Self::Low => (1280, 720),
            Self::Standard => (1920, 1080),
            Self::High => (2560, 1440),
            Self::Ultra => (3840, 2160),
        }
    }

    pub fn fit_dimensions(self, width: u32, height: u32) -> (u32, u32) {
        let (long, short) = self.max_dimensions();
        let (max_w, max_h) = if width >= height {
            (long, short)
        } else {
            (short, long)
        };
        let scale = (f64::from(max_w) / f64::from(width.max(1)))
            .min(f64::from(max_h) / f64::from(height.max(1)))
            .min(1.);
        (
            ((f64::from(width) * scale) as u32 & !1).max(2),
            ((f64::from(height) * scale) as u32 & !1).max(2),
        )
    }

    pub fn minimum_scale(self, width: u32, height: u32) -> f32 {
        (self.min_height() as f32 / width.min(height).max(1) as f32).min(1.)
    }

    /// Initial LAN budget grows with actual source pixels and refresh rate.
    /// The adaptive controller lowers it when delivery cannot sustain the rate.
    pub fn bitrate_kbps(self, width: u32, height: u32, fps: u8) -> u32 {
        let (w, h) = self.fit_dimensions(width, height);
        (u64::from(w) * u64::from(h) * u64::from(fps.max(1)) / 5000)
            .clamp(1000, u64::from(MAX_VIDEO_BITRATE_KBPS)) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ceilings_preserve_aspect_orientation_and_source_detail() {
        for q in VideoQuality::ALL {
            let (w, h) = q.max_dimensions();
            assert_eq!(q.fit_dimensions(7680, 4320), (w, h));
            assert_eq!(q.fit_dimensions(4320, 7680), (h, w));
            assert_eq!(q.fit_dimensions(640, 360), (640, 360));
            assert_eq!(
                (h as f32 * q.minimum_scale(w, h)).round() as u32,
                q.min_height()
            );
        }
        assert_eq!(VideoQuality::Ultra.fit_dimensions(5120, 1440), (3840, 1080));
        assert!(VideoQuality::Ultra.bitrate_kbps(3840, 2160, 120) > 150_000);
    }
}

//! removent-media-capture: ScreenCaptureKit screen frame and system audio capture.

pub mod health;
mod pixels;
pub mod quartz;
pub mod sck;
pub mod sck_ffi;

pub use pixels::CapturedPixels;

pub use sck::{AudioFrame, CaptureError, SckCapture, start_display_capture};

/// Dimensions travel with pixels, including the first frame after display changes.
#[derive(Debug)]
pub struct CapturedVideoFrame {
    pub pixels: CapturedPixels,
    pub pts_us: i64,
    pub width: usize,
    pub height: usize,
    /// Present on every frame so a restart survives latest-frame coalescing.
    pub generation: u64,
}

impl CapturedVideoFrame {
    pub fn new(bgra: Vec<u8>, width: usize, height: usize, pts_us: i64) -> Self {
        Self {
            pixels: CapturedPixels::Bgra(std::sync::Arc::new(bgra)),
            width,
            height,
            pts_us,
            generation: 0,
        }
    }
}

pub(crate) fn next_capture_generation() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static GENERATION: AtomicU64 = AtomicU64::new(1);
    GENERATION.fetch_add(1, Ordering::Relaxed)
}

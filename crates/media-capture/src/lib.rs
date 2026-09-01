//! removent-media-capture: ScreenCaptureKit screen frame and system audio capture.

pub mod sck;
pub mod sck_ffi;

pub use sck::{AudioFrame, CaptureError, SckCapture, start_display_capture};

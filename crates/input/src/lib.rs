//! removent-input: keyboard/mouse injection, clipboard, and display geometry (macOS).

pub mod clipboard;
pub mod display;
pub mod keyboard;
pub mod mouse;

pub use clipboard::{ClipboardError, change_count, read, read_text, write_text};
pub use display::DisplayGeometry;
pub use keyboard::{UNICODE_ONLY_VK, build_key, inject_key, inject_scroll_pixels, modifier_of_vk};
pub use mouse::inject_mouse;

/// Accessibility (TCC) preflight: whether this process is trusted to post
/// CGEvents. Never prompts; intended for logging a warning when a session
/// starts without the permission (injection would fail silently otherwise).
#[cfg(target_os = "macos")]
pub fn accessibility_trusted() -> bool {
    // SAFETY: a NULL options dictionary is documented as "check only, no prompt".
    unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) }
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
}

// The crate is macOS-only in practice; the stub keeps it compiling elsewhere.
#[cfg(not(target_os = "macos"))]
pub fn accessibility_trusted() -> bool {
    true
}

use removent_proto::DisplayInfo as ProtoDisplay;

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("injection failed: {0}")]
    Injection(String),
    #[error("clipboard: {0}")]
    Clipboard(#[from] ClipboardError),
}

/// Current display list (protocol DisplayListUpdate payload).
pub fn display_list() -> Vec<ProtoDisplay> {
    DisplayGeometry::all()
        .into_iter()
        .map(|g| ProtoDisplay {
            id: u64::from(g.id),
            w_px: g.width_px as u32,
            h_px: g.height_px as u32,
            scale: g.scale as f32,
            dpi: (96.0 * g.scale) as u32,
            is_main: g.is_main,
        })
        .collect()
}

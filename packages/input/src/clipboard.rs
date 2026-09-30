//! Clipboard read/write (NSPasteboard general).
//!
//! Supported formats correspond to ClipFormat in protocol.md §6.3
//! ClipboardSync: TextUtf8 / Rtf / Html / Png; FileRefs is handled by the file
//! transfer subsystem.

use objc2_app_kit::{NSPasteboard, NSPasteboardType};
use objc2_foundation::NSString;

#[derive(Debug, thiserror::Error)]
pub enum ClipboardError {
    #[error("pasteboard unavailable")]
    Unavailable,
    #[error("unsupported format for this operation")]
    UnsupportedFormat,
    #[error("no matching content on pasteboard")]
    NoContent,
}

fn uti_for(format: removent_proto::ClipFormat) -> &'static NSPasteboardType {
    use removent_proto::ClipFormat as F;
    // SAFETY: system constant strings, valid for the process lifetime.
    unsafe {
        match format {
            F::TextUtf8 => objc2_app_kit::NSPasteboardTypeString,
            F::Rtf => objc2_app_kit::NSPasteboardTypeRTF,
            F::Html => objc2_app_kit::NSPasteboardTypeHTML,
            F::Png => objc2_app_kit::NSPasteboardTypePNG,
            F::FileRefs => objc2_app_kit::NSPasteboardTypeFileURL,
        }
    }
}

/// Current pasteboard changeCount (basis for loop prevention / dedup,
/// architecture.md §7.3).
pub fn change_count() -> Result<u64, ClipboardError> {
    let pb = NSPasteboard::generalPasteboard();
    Ok(pb.changeCount() as u64)
}

/// Reads content in the given format.
pub fn read(format: removent_proto::ClipFormat) -> Result<Vec<u8>, ClipboardError> {
    if matches!(format, removent_proto::ClipFormat::FileRefs) {
        return Err(ClipboardError::UnsupportedFormat);
    }
    let pb = NSPasteboard::generalPasteboard();
    let data = pb.dataForType(uti_for(format));
    match data {
        Some(d) => Ok(d.to_vec()),
        None => Err(ClipboardError::NoContent),
    }
}

/// Convenience method for reading UTF-8 text.
pub fn read_text() -> Result<String, ClipboardError> {
    let bytes = read(removent_proto::ClipFormat::TextUtf8)?;
    String::from_utf8(bytes).map_err(|_| ClipboardError::NoContent)
}

/// Convenience method for writing text.
pub fn write_text(text: &str) -> Result<(), ClipboardError> {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let s = NSString::from_str(text);
    if !pb.setString_forType(&s, uti_for(removent_proto::ClipFormat::TextUtf8)) {
        return Err(ClipboardError::Unavailable);
    }
    Ok(())
}

use std::sync::{Mutex, OnceLock};
#[derive(Debug, thiserror::Error)]
pub enum ClipboardError {
    #[error("clipboard unavailable")]
    Unavailable,
    #[error("unsupported clipboard format")]
    UnsupportedFormat,
    #[error("clipboard has no text")]
    NoContent,
}
fn with<T>(
    f: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>,
) -> Result<T, ClipboardError> {
    static CLIP: OnceLock<Mutex<Option<arboard::Clipboard>>> = OnceLock::new();
    let mut clip = CLIP
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| ClipboardError::Unavailable)?;
    if clip.is_none() {
        *clip = Some(arboard::Clipboard::new().map_err(|_| ClipboardError::Unavailable)?);
    }
    f(clip.as_mut().unwrap()).map_err(|_| ClipboardError::NoContent)
}
pub fn change_count() -> Result<u64, ClipboardError> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    read_text()?.hash(&mut h);
    Ok(h.finish())
}
pub fn read(format: removent_proto::ClipFormat) -> Result<Vec<u8>, ClipboardError> {
    if format != removent_proto::ClipFormat::TextUtf8 {
        return Err(ClipboardError::UnsupportedFormat);
    }
    Ok(read_text()?.into_bytes())
}
pub fn read_text() -> Result<String, ClipboardError> {
    with(|c| c.get_text())
}
pub fn write_text(text: &str) -> Result<(), ClipboardError> {
    with(|c| c.set_text(text))
}

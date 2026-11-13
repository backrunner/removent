use enigo::{Enigo, Settings};
use std::sync::{Mutex, OnceLock};
pub fn available() -> bool {
    with(|_| Ok(())).is_ok()
}
pub fn with<T>(f: impl FnOnce(&mut Enigo) -> Result<T, enigo::InputError>) -> Result<T, String> {
    // XWayland input cannot control native Wayland windows. Fail explicitly.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return Err(
            "Remote input requires an X11 desktop session; Wayland portal input is not available"
                .into(),
        );
    }
    static INPUT: OnceLock<Mutex<Option<Enigo>>> = OnceLock::new();
    let mut input = INPUT
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|e| e.to_string())?;
    if input.is_none() {
        *input = Some(Enigo::new(&Settings::default()).map_err(|e| e.to_string())?);
    }
    f(input.as_mut().unwrap()).map_err(|e| e.to_string())
}

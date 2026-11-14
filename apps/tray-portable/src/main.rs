#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(any(windows, target_os = "linux"))]
rust_i18n::i18n!("locales");
#[cfg(any(windows, target_os = "linux"))]
mod dialogs;
#[cfg(any(windows, target_os = "linux"))]
mod prompts;
#[cfg(any(windows, target_os = "linux"))]
mod tray;
#[cfg(any(windows, target_os = "linux"))]
fn main() -> anyhow::Result<()> {
    tray::run()
}
#[cfg(not(any(windows, target_os = "linux")))]
fn main() {
    eprintln!("macOS uses the Swift RemoventTray helper");
}

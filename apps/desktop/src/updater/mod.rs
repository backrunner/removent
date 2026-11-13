//! Desktop event bridge. Download, validation and recovery live outside the UI.
use crate::engine::UiEvent;
use removent_core::DataPaths;
pub use removent_updater::{UpdatePolicy, UpdateShared, UpdateStatus};
use std::sync::{Arc, Mutex, mpsc::Sender};

fn forward(events: Sender<UiEvent>, action: impl FnOnce(Sender<removent_updater::UpdateEvent>)) {
    std::thread::scope(|scope| {
        let (tx, rx) = std::sync::mpsc::channel();
        scope.spawn(move || {
            for removent_updater::UpdateEvent::UpdateStatus(status) in rx {
                let _ = events.send(UiEvent::UpdateStatus(status));
            }
        });
        action(tx);
    });
}
pub fn run_check(manual: bool, shared: Arc<Mutex<UpdateShared>>, events: Sender<UiEvent>) {
    forward(events, |tx| removent_updater::run_check(manual, shared, tx));
}
pub fn run_download(
    generation: u64,
    paths: DataPaths,
    shared: Arc<Mutex<UpdateShared>>,
    events: Sender<UiEvent>,
) {
    forward(events, |tx| {
        removent_updater::run_download(generation, paths, shared, tx)
    });
}
pub fn run_install(
    generation: u64,
    shared: Arc<Mutex<UpdateShared>>,
    events: Sender<UiEvent>,
    paths: DataPaths,
) {
    forward(events, |tx| {
        removent_updater::run_install(generation, shared, tx, paths)
    });
}

#[cfg(target_os = "macos")]
pub use removent_updater::refresh_stale_tray;

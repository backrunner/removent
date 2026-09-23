//! Persist an explicit process stop across management clients and logins.
//! This is separate from host_enabled: a live daemon may have sharing disabled.

use crate::DataPaths;
use std::io;

pub fn is_stopped(paths: &DataPaths) -> io::Result<bool> {
    paths.run_dir().join("service-stopped").try_exists()
}

pub fn set_stopped(paths: &DataPaths, stopped: bool) -> anyhow::Result<()> {
    let file = paths.run_dir().join("service-stopped");
    if stopped {
        std::fs::create_dir_all(paths.run_dir())?;
        crate::settings::atomic_write(&file, b"Stopped by user\n")?;
    } else {
        match std::fs::remove_file(file) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            result => result?,
        }
    }
    Ok(())
}

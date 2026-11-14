//! Per-data-directory desktop ownership and activation from a second launch.
//! Activation contains no command or address: it can only raise the existing window.
use crate::{CoreError, DataDirLock, DataPaths, Result};
use std::{
    io::Read,
    sync::{Arc, mpsc},
    time::Duration,
};

pub struct DesktopInstance {
    _lock: DataDirLock,
    _life: Arc<()>,
    activations: mpsc::Receiver<()>,
}

fn signal_path(paths: &DataPaths) -> std::path::PathBuf {
    paths.run_dir().join("desktop-activate")
}

fn read_signal(paths: &DataPaths) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(signal_path(paths))
        .ok()?
        .take(128)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

impl DesktopInstance {
    /// The second launch asks the owner to activate, then exits without another engine.
    pub fn claim(paths: &DataPaths) -> Result<Option<Self>> {
        paths.ensure_layout()?;
        let lock = match DataDirLock::acquire_at(&paths.run_dir().join("desktop.lock")) {
            Ok(lock) => lock,
            Err(CoreError::AlreadyRunning) => {
                crate::settings::atomic_write(
                    &signal_path(paths),
                    format!("{:032x}", rand::random::<u128>()).as_bytes(),
                )?;
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let life = Arc::new(());
        let alive = Arc::downgrade(&life);
        let paths = paths.clone();
        let mut previous = read_signal(&paths);
        let (send, activations) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            while alive.strong_count() > 0 {
                std::thread::sleep(Duration::from_millis(150));
                let next = read_signal(&paths);
                if next.is_some() && next != previous {
                    if matches!(send.try_send(()), Err(mpsc::TrySendError::Disconnected(_))) {
                        break;
                    }
                    previous = next;
                }
            }
        });
        Ok(Some(Self {
            _lock: lock,
            _life: life,
            activations,
        }))
    }

    pub fn take_activation(&self) -> bool {
        self.activations.try_iter().count() > 0
    }

    pub fn is_running(paths: &DataPaths) -> bool {
        matches!(
            DataDirLock::acquire_at(&paths.run_dir().join("desktop.lock")),
            Err(CoreError::AlreadyRunning)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn second_launch_activates_only_its_data_directory_and_owner_can_restart() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let paths = DataPaths {
            root: a.path().into(),
        };
        let other_paths = DataPaths {
            root: b.path().into(),
        };
        let first = DesktopInstance::claim(&paths).unwrap().unwrap();
        let other = DesktopInstance::claim(&other_paths).unwrap().unwrap();
        assert!(DesktopInstance::is_running(&paths));
        assert!(DesktopInstance::claim(&paths).unwrap().is_none());
        first
            .activations
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert!(!other.take_activation());
        drop(first);
        assert!(!DesktopInstance::is_running(&paths));
        assert!(DesktopInstance::claim(&paths).unwrap().is_some());
    }
}

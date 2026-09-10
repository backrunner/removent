//! Settings persistence (settings.toml, FR-60).

use crate::error::Result;
use crate::paths::DataPaths;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub device_name: String,
    pub language: Language,
    pub theme: Theme,
    /// Controlled-side admission mode (FR-06).
    pub admission: AdmissionMode,
    /// Service master switch (toggleable from the menu bar).
    pub host_enabled: bool,
    pub host_port: u16,
    /// Optional legacy RFB/VNC listener for Apple Remote Desktop and other
    /// standard VNC clients. Disabled by default because RFB is LAN-only and
    /// its password authentication is intentionally legacy-compatible.
    pub vnc_enabled: bool,
    pub vnc_port: u16,
    /// Legacy client username retained for settings-file compatibility. New client
    /// connections use credentials from the connection dialog instead.
    pub vnc_username: String,
    /// Password for the local VNC compatibility listener. Empty selects RFB `None`.
    /// Remote client passwords are supplied per connection and are not persisted.
    pub vnc_password: String,
    pub audio_enabled_default: bool,
    /// Seconds to wait after the session window loses focus before clearing the remote-written clipboard; 0 = never clear (architecture.md §7.3).
    pub clipboard_clear_after_secs: u64,
    pub video_quality: QualityPreset,
    /// Update check (release.md §3).
    pub update_check_enabled: bool,
    pub update_endpoint: String,
    #[serde(skip)]
    pub loaded_from: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdmissionMode {
    AlwaysAsk,
    TrustedAuto,
    DenyAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    System,
    ZhCn,
    En,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    System,
    Dark,
    Light,
}

/// FR-15 quality presets: the Auto preset's clamp upper bound (kbps, fps, scale).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QualityPreset {
    Auto,
    Smooth,
    Balanced,
    HighQuality,
    Extreme,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            device_name: default_device_name(),
            language: Language::System,
            theme: Theme::System,
            admission: AdmissionMode::AlwaysAsk,
            host_enabled: true,
            host_port: removent_proto::DEFAULT_PORT,
            vnc_enabled: false,
            vnc_port: 5900,
            vnc_username: String::new(),
            vnc_password: String::new(),
            audio_enabled_default: true,
            clipboard_clear_after_secs: 60,
            video_quality: QualityPreset::Auto,
            update_check_enabled: true,
            update_endpoint: String::new(),
            loaded_from: None,
        }
    }
}

fn default_device_name() -> String {
    if let Ok(name) = std::env::var("REMOVENT_DEVICE_NAME") {
        return name;
    }
    // System hostname (macOS: same source as `scutil --get LocalHostName`).
    let mut buf = [0u8; 256];
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc == 0 {
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        let name = String::from_utf8_lossy(&buf[..end]).trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }
    "Mac".to_string()
}

impl Settings {
    /// Read-modify-write transaction shared by the daemon and settings UI.
    pub fn update(paths: &DataPaths, edit: impl FnOnce(&mut Self)) -> Result<Self> {
        let _lock = crate::DataDirLock::acquire_blocking(&paths.root.join(".settings.lock"))?;
        let mut next = Self::load(paths)?;
        edit(&mut next);
        next.save(paths)?;
        Ok(next)
    }

    pub fn load(paths: &DataPaths) -> Result<Self> {
        let file = paths.settings_file();
        let mut s: Settings = match std::fs::read_to_string(&file) {
            Ok(txt) => match toml::from_str(&txt) {
                Ok(s) => s,
                Err(e) => {
                    // Never silently reset a corrupt file: rename it to
                    // settings.toml.bak (replacing an older backup) so the user's
                    // original content survives the next save overwriting the file.
                    let bak = file.with_extension("toml.bak");
                    if let Err(re) = std::fs::rename(&file, &bak) {
                        return Err(re.into());
                    }
                    // TOML error Display includes source lines, potentially a VNC password.
                    tracing::warn!(
                        byte_offset = e.span().map(|span| span.start),
                        "settings.toml is corrupt; backed up to settings.toml.bak, using defaults"
                    );
                    Self::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(e.into()),
        };
        s.loaded_from = Some(file);
        Ok(s)
    }

    pub fn save(&self, paths: &DataPaths) -> Result<()> {
        paths.ensure_layout()?;
        let file = paths.settings_file();
        let txt = toml::to_string_pretty(self)?;
        atomic_write(&file, txt.as_bytes())?;
        Ok(())
    }
}

/// Atomic write via tmp + rename to avoid half-written files.
pub fn atomic_write(path: &PathBuf, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    // Each writer owns a separate sibling file. A shared .tmp can be renamed
    // while another writer is still modifying it, corrupting the final file.
    let tmp = path.with_extension(format!("{:032x}.tmp", rand::random::<u128>()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    let result = (|| -> std::io::Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn concurrent_setting_edits_preserve_service_switch() {
        let dir = tempdir().unwrap();
        let paths = DataPaths {
            root: dir.path().to_owned(),
        };
        Settings::default().save(&paths).unwrap();
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                barrier.wait();
                Settings::update(&paths, |s| s.host_enabled = false).unwrap();
            });
            scope.spawn(|| {
                barrier.wait();
                Settings::update(&paths, |s| s.device_name = "Edited in main window".into())
                    .unwrap();
            });
        });
        let result = Settings::load(&paths).unwrap();
        assert!(!result.host_enabled);
        assert_eq!(result.device_name, "Edited in main window");
    }

    #[test]
    fn concurrent_writers_publish_complete_private_files() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::{Arc, Barrier};
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let barrier = Arc::new(Barrier::new(8));
        std::thread::scope(|scope| {
            for byte in b'a'..=b'h' {
                let path = &path;
                let barrier = barrier.clone();
                scope.spawn(move || {
                    let payload = vec![byte; 64 * 1024];
                    barrier.wait();
                    for _ in 0..10 {
                        atomic_write(path, &payload).unwrap();
                        let visible = std::fs::read(path).unwrap();
                        assert_eq!(visible.len(), payload.len());
                        assert!(visible.iter().all(|b| *b == visible[0]));
                    }
                });
            }
        });
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_atomic_save_cleans_its_temporary_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::create_dir(&path).unwrap();
        assert!(atomic_write(&path, b"secret").is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn read_error_and_failed_corrupt_backup_are_reported() {
        let dir = tempdir().unwrap();
        let paths = DataPaths {
            root: dir.path().to_owned(),
        };
        paths.ensure_layout().unwrap();
        std::fs::create_dir(paths.settings_file()).unwrap();
        assert!(Settings::load(&paths).is_err());
        std::fs::remove_dir(paths.settings_file()).unwrap();
        std::fs::write(paths.settings_file(), "[[[ invalid").unwrap();
        std::fs::create_dir(paths.settings_file().with_extension("toml.bak")).unwrap();
        assert!(Settings::load(&paths).is_err());
        assert_eq!(
            std::fs::read_to_string(paths.settings_file()).unwrap(),
            "[[[ invalid"
        );
    }

    #[test]
    fn defaults_roundtrip_preserve_semantics() {
        let dir = tempdir().unwrap();
        let p = DataPaths {
            root: dir.path().to_path_buf(),
        };

        let s = Settings::load(&p).unwrap();
        assert_eq!(s.admission, AdmissionMode::AlwaysAsk);
        assert_eq!(s.host_port, removent_proto::DEFAULT_PORT);
        assert!(!s.vnc_enabled);
        assert_eq!(s.vnc_port, 5900);
        assert!(s.vnc_username.is_empty());
        assert!(s.vnc_password.is_empty());

        let mut modified = s.clone();
        modified.device_name = "Living Room Mac".into();
        modified.admission = AdmissionMode::TrustedAuto;
        modified.video_quality = QualityPreset::HighQuality;
        modified.vnc_enabled = true;
        modified.vnc_username = "alice".into();
        modified.vnc_password = "test-password".into();
        modified.save(&p).unwrap();

        let reloaded = Settings::load(&p).unwrap();
        assert_eq!(reloaded, modified);
    }

    #[test]
    fn unknown_fields_tolerated() {
        let dir = tempdir().unwrap();
        let p = DataPaths {
            root: dir.path().to_path_buf(),
        };
        p.ensure_layout().unwrap();
        std::fs::write(
            p.settings_file(),
            "device_name = \"x\"\nsome_future_field = 42\n",
        )
        .unwrap();
        let s = Settings::load(&p).unwrap();
        assert_eq!(s.device_name, "x");
    }

    #[test]
    fn corrupt_file_is_backed_up_not_silently_reset() {
        let dir = tempdir().unwrap();
        let p = DataPaths {
            root: dir.path().to_path_buf(),
        };
        p.ensure_layout().unwrap();
        let bad = "device_name = \"broken\"\n[[[ not toml";
        std::fs::write(p.settings_file(), bad).unwrap();

        // Load succeeds with defaults; the corrupt file is renamed to
        // settings.toml.bak instead of being silently discarded.
        let s = Settings::load(&p).unwrap();
        assert_eq!(s.admission, AdmissionMode::AlwaysAsk);
        assert!(!p.settings_file().exists());
        let bak = p.settings_file().with_extension("toml.bak");
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), bad);

        // The next save recreates settings.toml without clobbering the backup.
        s.save(&p).unwrap();
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), bad);
        let reloaded = Settings::load(&p).unwrap();
        assert_eq!(reloaded, s);

        // A second corruption replaces the older backup.
        std::fs::write(p.settings_file(), "not toml either {{{").unwrap();
        Settings::load(&p).unwrap();
        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            "not toml either {{{"
        );
    }
}

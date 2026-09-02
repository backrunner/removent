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
    /// macOS account name used by Apple Remote Desktop (RFB 003.889, types 30/35).
    pub vnc_username: String,
    /// VNC password. Empty selects RFB `None` for standard servers; Apple
    /// Remote Desktop uses this together with `vnc_username` for types 30/35.
    /// The value is stored only in settings.toml.
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
                        tracing::warn!(error = %re, path = %file.display(), "failed to back up corrupt settings file");
                    }
                    tracing::warn!(error = %e, "settings.toml is corrupt; backed up to settings.toml.bak, using defaults");
                    Self::default()
                }
            },
            Err(_) => Self::default(),
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
pub(crate) fn atomic_write(path: &PathBuf, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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

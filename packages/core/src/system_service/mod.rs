//! Administrator-managed host for the macOS LoginWindow session. Code and
//! authorization are copied into a root-owned installation, never read live
//! from user-writable paths by the privileged host.
use crate::{DataPaths, Settings};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

mod install;
mod security;
mod snapshot;
#[cfg(test)]
mod tests;

pub const ROOT: &str = "/Library/Application Support/Removent/LoginWindow";
pub const PLIST: &str = "/Library/LaunchAgents/com.alkinum.removent.loginwindow.plist";
pub const LABEL: &str = "com.alkinum.removent.loginwindow";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovedDevice {
    pub fingerprint: String,
    pub name: String,
    pub input: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemServiceStatus {
    pub installed: bool,
    pub enabled: bool,
    pub version: String,
    pub host_fingerprint: String,
    pub approved_devices: Vec<ApprovedDevice>,
}

pub fn status() -> Result<SystemServiceStatus> {
    status_at(Path::new(ROOT), Path::new(PLIST))
}

fn status_at(root: &Path, plist: &Path) -> Result<SystemServiceStatus> {
    if !root.try_exists()? {
        ensure!(
            !plist.try_exists()?,
            "LoginWindow registration has no installed host; reinstall it"
        );
        return Ok(SystemServiceStatus::default());
    }
    let manifest = root.join("status.json");
    security::verify_root_owned(&manifest)?;
    let mut status: SystemServiceStatus =
        serde_json::from_slice(&security::read_regular(&manifest)?)
            .context("LoginWindow installation needs to be updated")?;
    status.installed = true;
    status.enabled = plist.try_exists()?;
    if status.enabled {
        security::verify_root_owned(plist)?;
    }
    Ok(status)
}

pub fn current_approved_devices(paths: &DataPaths) -> Result<Vec<ApprovedDevice>> {
    Ok(snapshot::approved_devices(&snapshot::trusted_peers(paths)?))
}

/// The desktop calls the packaged CLI with macOS administrator authorization.
/// CLI callers can use sudo with an explicit REMOVENT_DATA_DIR.
pub fn enable(bundle: &Path, paths: &DataPaths) -> Result<()> {
    install::install(bundle, paths, true)
}

pub fn update(bundle: &Path, paths: &DataPaths) -> Result<()> {
    install::install(bundle, paths, false)
}

pub fn disable() -> Result<()> {
    install::disable()
}

pub fn installed_bundle(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    if !macos.ends_with("Contents/MacOS") {
        return None;
    }
    let bundle = macos.parent()?.parent()?;
    (bundle.extension()?.to_str()? == "app").then(|| bundle.to_path_buf())
}

/// Verify before presenting an administrator prompt, then reverify the staged
/// root-owned copy before publishing. An arbitrary signed third-party app is
/// not a valid system host.
pub fn verify_bundle(bundle: &Path) -> Result<()> {
    security::verify_bundle(bundle)
}

fn definition(root: &Path) -> String {
    let escape = |value: &str| {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>{LABEL}</string>
<key>AssociatedBundleIdentifiers</key><array><string>com.alkinum.removent</string></array>
<key>LimitLoadToSessionType</key><array><string>LoginWindow</string></array>
<key>ProgramArguments</key><array><string>{}/Removent.app/Contents/MacOS/removentd</string><string>--login-window</string></array>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><dict><key>PathState</key><dict><key>{}/data/run/service-stopped</key><false/></dict></dict>
<key>ThrottleInterval</key><integer>10</integer>
<key>ProcessType</key><string>Interactive</string>
<key>ExitTimeOut</key><integer>10</integer>
</dict></plist>
"#,
        escape(&root.to_string_lossy()),
        escape(&root.to_string_lossy())
    )
}

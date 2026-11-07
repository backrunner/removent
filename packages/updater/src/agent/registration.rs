use super::{io, preflight};
use anyhow::{Context, Result, ensure};
use removent_core::{DataDirLock, DataPaths};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

pub fn directory(paths: &DataPaths) -> PathBuf {
    paths.root.join("update-agent")
}
pub fn label(paths: &DataPaths) -> Result<String> {
    let digest = hex::encode(Sha256::digest(
        paths.root.canonicalize()?.as_os_str().as_encoded_bytes(),
    ));
    Ok(format!("com.alkinum.removent.update.{}", &digest[..12]))
}
fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Install a pinned recovery runtime OUTSIDE the replaceable application. A
/// candidate never overwrites the only working recovery executable. It reads
/// the installed bundle version, so it can install later repair releases even
/// when the desktop and host no longer launch.
pub fn install(paths: &DataPaths, bundle: &Path) -> Result<()> {
    ensure!(
        unsafe { libc::geteuid() } != 0,
        "The updater belongs to the logged-in user"
    );
    paths.ensure_layout()?;
    let directory = directory(paths);
    io::private_dir(&directory)?;
    let _lock = DataDirLock::acquire_at(&directory.join("registration.lock"))?;
    let bundle = bundle.canonicalize()?;
    preflight::verify(&bundle)?;
    let executable = directory.join("removent-update");
    if !executable.try_exists()? {
        let staged = tempfile::NamedTempFile::new_in(&directory)?;
        fs::copy(bundle.join("Contents/MacOS/removent-update"), staged.path())?;
        preflight::verify_component(staged.path(), "com.alkinum.removent.update")?;
        staged.as_file().sync_all()?;
        staged.persist(&executable)?;
        io::sync_dir(&directory)?;
    }
    preflight::verify_component(&executable, "com.alkinum.removent.update")?;
    io::checked(
        Command::new(&executable).arg("--self-test"),
        Duration::from_secs(10),
    )?;
    if !removent_core::update_gate::bundle_marker(&bundle)?.exists() {
        super::supervisor::stage(paths, &bundle)?;
    }
    let label = label(paths)?;
    let domain = format!("gui/{}", unsafe { libc::getuid() });
    let target = format!("{domain}/{label}");
    let config = directory.join("installation.json");
    if config.exists() {
        let registered: PathBuf = io::load(&config)?;
        ensure!(
            registered == bundle,
            "A different installation owns this updater; remove the old registration first"
        );
    } else {
        io::save(&config, &bundle)?;
    }
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME is unavailable")?);
    let plist = home.join(format!("Library/LaunchAgents/{label}.plist"));
    fs::create_dir_all(plist.parent().unwrap())?;
    let contents = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict>
<key>Label</key><string>{label}</string><key>ProgramArguments</key><array><string>{exe}</string><string>--supervise</string><string>{bundle}</string></array>
<key>EnvironmentVariables</key><dict><key>REMOVENT_DATA_DIR</key><string>{data}</string></dict>
<key>AssociatedBundleIdentifiers</key><array><string>com.alkinum.removent</string></array>
<key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>30</integer>
<key>ProcessType</key><string>Background</string><key>ExitTimeOut</key><integer>20</integer>
</dict></plist>"#,
        exe = xml(&executable.to_string_lossy()),
        bundle = xml(&bundle.to_string_lossy()),
        data = xml(&paths.root.canonicalize()?.to_string_lossy())
    );
    io::write(&plist, contents.as_bytes())?;
    let loaded = io::run(
        Command::new("/bin/launchctl").args(["print", &target]),
        Duration::from_secs(10),
    )?
    .status
    .success();
    if !loaded {
        io::checked(
            Command::new("/bin/launchctl").args(["enable", &target]),
            Duration::from_secs(10),
        )?;
        io::checked(
            Command::new("/bin/launchctl")
                .args(["bootstrap", &domain])
                .arg(plist),
            Duration::from_secs(15),
        )?;
    }
    Ok(())
}

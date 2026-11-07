use super::io;
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap, fs, os::unix::fs::MetadataExt, path::Path, process::Command,
    time::Duration,
};

pub const COMPONENTS: &[(&str, &str)] = &[
    ("", "com.alkinum.removent"),
    ("Contents/MacOS/removent", "com.alkinum.removent.desktop"),
    ("Contents/MacOS/removent-cli", "com.alkinum.removent.cli"),
    (
        "Contents/MacOS/removent-update",
        "com.alkinum.removent.update",
    ),
    (
        "Contents/Helpers/RemoventHost.app",
        "com.alkinum.removent.host",
    ),
    (
        "Contents/Helpers/RemoventTray.app",
        "com.alkinum.removent.tray",
    ),
    (
        "Contents/Helpers/RemoventSync.app",
        "com.alkinum.removent.sync",
    ),
];
pub fn verify_component(path: &Path, identifier: &str) -> Result<()> {
    let requirement = format!(
        "=anchor apple generic and identifier \"{identifier}\" and certificate leaf[subject.OU] = \"{}\" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists",
        crate::EXPECTED_TEAM_ID
    );
    io::checked(
        Command::new("/usr/bin/codesign")
            .args([
                "--verify",
                "--strict",
                "--all-architectures",
                "-R",
                &requirement,
            ])
            .arg(path),
        Duration::from_secs(30),
    )?;
    Ok(())
}
pub fn verify(bundle: &Path) -> Result<()> {
    crate::codesign_verify(bundle).map_err(anyhow::Error::msg)?;
    let root = bundle.canonicalize()?;
    for (path, id) in COMPONENTS {
        ensure!(
            bundle.join(path).canonicalize()?.starts_with(&root),
            "Signed component points outside the installation"
        );
        verify_component(&bundle.join(path), id)?;
    }
    Ok(())
}
fn plist(path: &Path) -> Result<serde_json::Value> {
    let output = io::checked(
        Command::new("/usr/bin/plutil")
            .args(["-convert", "json", "-o", "-"])
            .arg(path),
        Duration::from_secs(10),
    )?;
    Ok(serde_json::from_slice(&output.stdout)?)
}
fn permissions(bundle: &Path) -> Result<BTreeMap<String, serde_json::Value>> {
    let mut result = BTreeMap::new();
    for (path, _) in COMPONENTS {
        if path.is_empty() || path.ends_with(".app") {
            let info = plist(&bundle.join(path).join("Contents/Info.plist"))?;
            for (key, value) in info.as_object().context("Invalid bundle metadata")? {
                if key.ends_with("UsageDescription")
                    || key == "NSBonjourServices"
                    || key == "RemoventProtocolVersion"
                {
                    let value = if key.ends_with("UsageDescription") {
                        serde_json::Value::Bool(true)
                    } else {
                        value.clone()
                    };
                    result.insert(format!("{path}/{key}"), value);
                }
            }
        }
        let ent = io::checked(
            Command::new("/usr/bin/codesign")
                .args(["-d", "--entitlements", ":-"])
                .arg(bundle.join(path)),
            Duration::from_secs(10),
        )?;
        if !ent.stdout.is_empty() {
            let tmp = tempfile::NamedTempFile::new()?;
            fs::write(tmp.path(), ent.stdout)?;
            result.insert(format!("{path}/entitlements"), plist(tmp.path())?);
        }
    }
    Ok(result)
}
pub fn installation(bundle: &Path, candidate: &Path) -> Result<()> {
    ensure!(
        unsafe { libc::geteuid() } != 0,
        "Unattended installation cannot run as root"
    );
    // LoginWindow has an independent root-owned copy. Preserve that working
    // installation; its existing settings UI handles an explicit admin update.
    // Invalid system registration still fails closed before touching user code.
    let _system = removent_core::system_service::status()?;
    let meta = fs::symlink_metadata(bundle)?;
    ensure!(
        meta.is_dir() && meta.uid() == unsafe { libc::geteuid() },
        "This installation requires administrator approval"
    );
    verify(bundle)?;
    verify(candidate)?;
    ensure!(
        permissions(bundle)? == permissions(candidate)?,
        "This update changes macOS permissions or protocol compatibility; install it interactively"
    );
    let parent = bundle.parent().context("Missing application directory")?;
    // An actual same-directory create and rename catches ACL/read-only failures.
    let test = tempfile::Builder::new()
        .prefix(".removent-write-")
        .tempdir_in(parent)?;
    fs::write(test.path().join("a"), b"probe")?;
    fs::rename(test.path().join("a"), test.path().join("b"))?;
    Ok(())
}
pub fn copy(candidate: &Path, incoming: &Path) -> Result<()> {
    ensure!(
        !incoming.try_exists()?,
        "An unfinished staged installation needs recovery"
    );
    io::checked(
        Command::new("/usr/bin/ditto").arg(candidate).arg(incoming),
        Duration::from_secs(180),
    )?;
    verify(incoming)?;
    flush_bundle(incoming, incoming)?;
    io::sync_dir(incoming.parent().unwrap())
}

fn flush_bundle(path: &Path, root: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        ensure!(
            path.canonicalize()?.starts_with(root.canonicalize()?),
            "Bundle link escapes installation"
        );
    } else if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            flush_bundle(&entry?.path(), root)?;
        }
        io::sync_dir(path)?;
    } else {
        ensure!(meta.is_file(), "Unsupported special file in update");
        fs::File::open(path)?.sync_all()?;
    }
    Ok(())
}

//! Durable state and bounded subprocesses. No shell is involved in installation.
use anyhow::{Context, Result, ensure};
use serde::{Serialize, de::DeserializeOwned};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
    process::{Command, Output},
    time::Duration,
};

pub fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_dir() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o022 == 0,
        "Update directory is not owned exclusively by this user"
    );
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("Missing update state directory")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    sync_dir(parent)
}

pub fn save(path: &Path, value: &impl Serialize) -> Result<()> {
    write(path, &serde_json::to_vec(value)?)
}
pub fn load<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    ensure!(
        meta.is_file()
            && meta.len() <= 1024 * 1024
            && meta.uid() == unsafe { libc::geteuid() }
            && meta.nlink() == 1
            && meta.mode() & 0o022 == 0,
        "Unsafe update state file"
    );
    Ok(serde_json::from_reader(file)?)
}
pub fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => sync_dir(path.parent().unwrap()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

pub use removent_core::process::run_bounded as run;
pub fn checked(command: &mut Command, timeout: Duration) -> Result<Output> {
    let output = run(command, timeout)?;
    ensure!(
        output.status.success(),
        "{:?} failed: {}",
        command.get_program(),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output)
}

//! Restricted global LoginWindow-agent mode. Installed explicitly by an admin;
//! never consumes the user's writable identity/settings or executes user code.
use anyhow::{Result, ensure};
use removent_core::{DataPaths, Settings};
use std::{
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::Path,
};

unsafe extern "C" {
    fn acl_get_file(path: *const libc::c_char, kind: libc::c_int) -> *mut libc::c_void;
    fn acl_free(acl: *mut libc::c_void) -> libc::c_int;
}

fn no_extended_acl(path: &Path) -> Result<()> {
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    // ACL grants can allow writes even when POSIX mode bits look safe. This
    // restricted installation rejects all extended ACLs, including inherited ones.
    let acl = unsafe { acl_get_file(path.as_ptr(), 0x100) };
    if acl.is_null() {
        ensure!(
            std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT),
            "Could not verify LoginWindow ACL"
        );
        return Ok(());
    }
    unsafe {
        acl_free(acl);
    }
    anyhow::bail!("LoginWindow paths must not have extended ACLs");
}

pub const ROOT: &str = "/Library/Application Support/Removent/LoginWindow";

pub fn paths() -> Result<DataPaths> {
    ensure!(
        unsafe { libc::geteuid() } == 0,
        "LoginWindow mode requires its installed system agent"
    );
    let executable = std::env::current_exe()?;
    ensure!(
        executable.starts_with(Path::new(ROOT).join("Removent.app")),
        "LoginWindow executable is not installed"
    );
    verify_root_owned(&executable)?;
    let paths = DataPaths {
        root: Path::new(ROOT).join("data"),
    };
    verify_root_owned(&paths.root)?;
    for file in [
        paths.device_key(),
        paths.device_cert(),
        paths.peers_file(),
        paths.settings_file(),
    ] {
        verify_root_owned(&file)?;
    }
    let relay = paths.root.join("relay-host.toml");
    if relay.exists() {
        verify_root_owned(&relay)?;
    }
    Ok(paths)
}

pub fn restrict(settings: &mut Settings) {
    settings.window_server_capture = true;
    settings.paired_only = true;
    settings.vnc_enabled = false;
    settings.audio_enabled_default = false;
    settings.update_check_enabled = false;
}

/// Check every ancestor; a safe leaf inside a replaceable directory is unsafe.
fn verify_root_owned(path: &Path) -> Result<()> {
    for component in path.ancestors() {
        let meta = std::fs::symlink_metadata(component)?;
        no_extended_acl(component)?;
        ensure!(
            !meta.file_type().is_symlink() && meta.uid() == 0 && meta.mode() & 0o022 == 0,
            "LoginWindow files and parents must be root owned, non-symlink and not group/world writable"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn policy_cannot_enable_legacy_or_pairing_services() {
        let mut settings = Settings {
            vnc_enabled: true,
            paired_only: false,
            ..Settings::default()
        };
        restrict(&mut settings);
        assert!(settings.paired_only && settings.window_server_capture);
        assert!(
            !settings.vnc_enabled
                && !settings.audio_enabled_default
                && !settings.update_check_enabled
        );
    }
    #[test]
    fn writable_user_paths_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(verify_root_owned(dir.path()).is_err());
    }

    #[test]
    fn acl_grants_are_not_hidden_by_safe_mode_bits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        std::fs::write(&path, b"fixture").unwrap();
        no_extended_acl(&path).unwrap();
        let status = std::process::Command::new("/bin/chmod")
            .arg("+a")
            .arg("everyone allow write")
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(no_extended_acl(&path).is_err());
    }
}

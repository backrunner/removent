use super::*;
use std::io::Read;
use std::os::unix::{
    ffi::OsStrExt,
    fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
};
use std::process::Command;

// codesign treats a requirement without '=' as a file path.
const SIGNING_REQUIREMENT: &str = "=anchor apple generic and identifier \"com.alkinum.removent\" and certificate leaf[subject.OU] = \"PB8H83VL3Z\" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists";

unsafe extern "C" {
    fn acl_get_file(path: *const libc::c_char, kind: libc::c_int) -> *mut libc::c_void;
    fn acl_free(acl: *mut libc::c_void) -> libc::c_int;
}

pub(super) fn verify_root_owned(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        let metadata = std::fs::symlink_metadata(ancestor)?;
        ensure!(
            !metadata.file_type().is_symlink()
                && metadata.uid() == 0
                && metadata.mode() & 0o022 == 0,
            "System service paths must be root owned and not writable by other users"
        );
        let name = std::ffi::CString::new(ancestor.as_os_str().as_bytes())?;
        let acl = unsafe { acl_get_file(name.as_ptr(), 0x100) };
        if !acl.is_null() {
            unsafe {
                acl_free(acl);
            }
            anyhow::bail!("System service paths must not have extended ACLs");
        }
        ensure!(
            std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT),
            "Could not verify system service ACL"
        );
    }
    Ok(())
}

pub(super) fn safe_directory(path: &Path) -> Result<()> {
    if !path.try_exists()? {
        safe_directory(path.parent().context("Invalid system directory")?)?;
        std::fs::create_dir(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    verify_root_owned(path)
}

pub(super) fn read_regular(path: &Path) -> Result<Vec<u8>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    ensure!(
        file.metadata()?.is_file(),
        "Expected a regular configuration file"
    );
    let mut bytes = Vec::new();
    file.take(1_048_577).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 1_048_576, "Configuration file is too large");
    Ok(bytes)
}

pub(super) fn checked(program: &str, args: &[&std::ffi::OsStr]) -> Result<()> {
    let output = Command::new(program).args(args).output()?;
    ensure!(
        output.status.success(),
        "{program}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

pub(super) fn verify_bundle(bundle: &Path) -> Result<()> {
    checked(
        "/usr/bin/codesign",
        &[
            "--verify".as_ref(),
            "--deep".as_ref(),
            "--strict".as_ref(),
            "-R".as_ref(),
            SIGNING_REQUIREMENT.as_ref(),
            bundle.as_os_str(),
        ],
    )?;
    checked(
        "/usr/sbin/spctl",
        &[
            "--assess".as_ref(),
            "--type".as_ref(),
            "execute".as_ref(),
            bundle.as_os_str(),
        ],
    )
}

pub(super) fn secure_bundle_tree(bundle: &Path, path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        ensure!(
            path == bundle.join("Contents/MacOS/removentd")
                && std::fs::read_link(path)?
                    == Path::new("../Helpers/RemoventHost.app/Contents/MacOS/removentd"),
            "Unexpected system host symlink"
        );
        let target = bundle.join("Contents/Helpers/RemoventHost.app/Contents/MacOS/removentd");
        ensure!(
            std::fs::symlink_metadata(target)?.is_file(),
            "Invalid system host entry point"
        );
        return Ok(());
    }
    ensure!(
        metadata.uid() == 0,
        "Copied system host must be owned by root"
    );
    ensure!(
        metadata.is_file() || metadata.is_dir(),
        "Unexpected system host file type"
    );
    std::fs::set_permissions(
        path,
        // File mode bits are not a code-signing boundary. Never turn a copied
        // executable into a setuid/setgid root program.
        std::fs::Permissions::from_mode(system_code_mode(metadata.mode())),
    )?;
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path)? {
            secure_bundle_tree(bundle, &entry?.path())?;
        }
    }
    Ok(())
}

fn system_code_mode(mode: u32) -> u32 {
    mode & !0o7022
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_requirement_is_accepted_by_macos_as_inline_source() {
        checked(
            "/usr/bin/csreq",
            &["-r".as_ref(), SIGNING_REQUIREMENT.as_ref(), "-t".as_ref()],
        )
        .unwrap();
    }

    #[test]
    fn copying_code_never_creates_setuid_or_other_user_writable_root_code() {
        for mode in 0..=0o7777 {
            let secured = system_code_mode(mode);
            assert_eq!(secured & 0o7022, 0);
            assert_eq!(secured & 0o755, mode & 0o755);
        }
    }
}

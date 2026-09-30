use super::*;

/// Download to a `.part` file; `resume` continues a partial download (`-C -`).
pub(super) fn curl_download(url: &str, part: &Path, resume: bool) -> bool {
    let mut cmd = Command::new("/usr/bin/curl");
    cmd.arg("-fSL")
        .args([
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "15",
            "--speed-limit",
            "1024",
            "--speed-time",
            "30",
            "--max-filesize",
            "1073741824",
        ])
        .arg("--max-time")
        .arg("600")
        .arg("-o")
        .arg(part);
    if resume {
        cmd.arg("-C").arg("-");
    }
    cmd.arg("--url")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub(super) fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| t!("update.err.download", err = e.to_string()).to_string())?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .map_err(|e| t!("update.err.download", err = e.to_string()).to_string())?;
    Ok(hex::encode(hasher.finalize()))
}

/// The release zip holds a single top-level `Removent.app`.
pub(super) fn find_app_bundle(stage: &Path) -> Option<PathBuf> {
    let app = stage.join("Removent.app");
    let metadata = std::fs::symlink_metadata(&app).ok()?;
    (metadata.is_dir() && !metadata.file_type().is_symlink()).then_some(app)
}

pub(super) fn release_codesign_requirement() -> String {
    // codesign treats -R as a filename unless the expression starts with '='.
    format!(
        "=anchor apple generic and identifier \"com.alkinum.removent\" and certificate leaf[subject.OU] = \"{EXPECTED_TEAM_ID}\" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists"
    )
}

pub(super) fn codesign_verify(app: &Path) -> Result<(), String> {
    let requirement = release_codesign_requirement();
    let ok = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict", "-R", &requirement])
        .arg(app)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return Err(t!("update.err.codesign").to_string());
    }
    Ok(())
}

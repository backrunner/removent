use super::*;

/// Download the available update and run the triple verification. Runs on a
/// blocking thread; on success the state becomes ReadyToInstall.
pub fn run_download(
    generation: u64,
    paths: DataPaths,
    shared: Arc<Mutex<UpdateShared>>,
    events: std::sync::mpsc::Sender<UiEvent>,
) {
    // Concurrency guard (double-click on "Download and Install"): claim the
    // Available → Downloading transition inside one lock; a second click sees
    // a non-Available state and returns instead of racing the same .part file.
    let m = {
        let mut s = shared.lock().unwrap();
        if s.generation != generation {
            return;
        }
        let UpdateStatus::Available { version, .. } = &s.status else {
            return;
        };
        let Some(m) = s.manifest.clone() else {
            return;
        };
        if !eligible_upgrade(s.policy.channel, &m.version, env!("CARGO_PKG_VERSION")) {
            s.status = UpdateStatus::Failed(t!("update.err.not_upgrade").to_string());
            let _ = events.send(UiEvent::UpdateStatus(s.status.clone()));
            return;
        }
        let version = version.clone();
        s.status = UpdateStatus::Downloading {
            version: version.clone(),
        };
        let _ = events.send(UiEvent::UpdateStatus(s.status.clone()));
        m
    };
    let version = m.version.clone();
    match download_and_stage(&paths, &m, &shared, &events) {
        Ok(app) => {
            shared.lock().unwrap().staged_app = Some(app);
            set_status(&shared, &events, UpdateStatus::ReadyToInstall { version });
        }
        Err(reason) => set_status(&shared, &events, UpdateStatus::Failed(reason)),
    }
}

pub(super) fn download_and_stage(
    paths: &DataPaths,
    m: &UpdateManifest,
    shared: &Arc<Mutex<UpdateShared>>,
    events: &std::sync::mpsc::Sender<UiEvent>,
) -> Result<PathBuf, String> {
    let dir = paths.update_cache();
    std::fs::create_dir_all(&dir)
        .map_err(|e| t!("update.err.download", err = e.to_string()).to_string())?;
    let part = dir.join("update.zip.part");
    let zip = dir.join("update.zip");
    // A .part left over from an older release can never match the new
    // manifest's hash when resumed (-C -): allow one automatic from-scratch
    // retry on hash mismatch instead of failing the download outright.
    let mut stale_resume_retry = part.exists();
    loop {
        // Resume an interrupted download; when the server cannot honor the
        // range (or the partial file is stale), restart from scratch once.
        if !curl_download(&m.url, &part, true) {
            let _ = std::fs::remove_file(&part);
            if !curl_download(&m.url, &part, false) {
                return Err(t!("update.err.download", err = "curl").to_string());
            }
        }
        std::fs::rename(&part, &zip)
            .map_err(|e| t!("update.err.download", err = e.to_string()).to_string())?;

        set_status(
            shared,
            events,
            UpdateStatus::Verifying {
                version: m.version.clone(),
            },
        );

        // Verification (1/3): the downloaded bytes must match the manifest hash.
        // Any mismatch discards the download — it is never executed (release.md §3.2).
        let digest = sha256_file(&zip)?;
        if digest.eq_ignore_ascii_case(m.sha256.trim()) {
            break;
        }
        let _ = std::fs::remove_file(&zip);
        if !stale_resume_retry {
            return Err(t!("update.err.hash_mismatch").to_string());
        }
        stale_resume_retry = false;
        // Loop back: the .part is gone, so the retry downloads from scratch.
    }
    // Verification (2/3): re-check the release signature over the payload
    // (covers version/url/sha256, so the hash we just matched is authentic).
    let key = release_verifying_key().ok_or_else(|| t!("update.err.signature").to_string())?;
    if !verify_manifest_signature(m, &key) {
        let _ = std::fs::remove_file(&zip);
        return Err(t!("update.err.signature").to_string());
    }

    // Verification (3/3): unpack and check the Apple code signature.
    let stage = dir.join("stage");
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage)
        .map_err(|e| t!("update.err.unpack", err = e.to_string()).to_string())?;
    let unpacked = Command::new("ditto")
        .arg("-x")
        .arg("-k")
        .arg(&zip)
        .arg(&stage)
        .status()
        .map_err(|e| t!("update.err.unpack", err = e.to_string()).to_string())?;
    if !unpacked.success() {
        return Err(t!("update.err.unpack", err = "ditto").to_string());
    }
    let app = find_app_bundle(&stage)
        .ok_or_else(|| t!("update.err.unpack", err = "no .app").to_string())?;
    codesign_verify(&app)?;
    if bundle_version(&app)? != m.version {
        return Err(t!(
            "update.err.unpack",
            err = "bundle version does not match signed manifest"
        )
        .to_string());
    }
    Ok(app)
}

pub(super) fn bundle_version(app: &Path) -> Result<String, String> {
    let version = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :RemoventReleaseVersion"])
        .arg(app.join("Contents/Info.plist"))
        .output()
        .map_err(|e| t!("update.err.unpack", err = e.to_string()).to_string())?;
    let value = String::from_utf8_lossy(&version.stdout).trim().to_string();
    if !version.status.success() || parse_version(&value).is_none() {
        return Err(t!("update.err.unpack", err = "invalid bundle release version").to_string());
    }
    Ok(value)
}

// ---- swap + relaunch ----

/// daemon IPC request outlet (shared with the engine); used to stop the old
/// daemon before relaunching after an update.
pub(super) type DaemonReq =
    Arc<Mutex<Option<tokio::sync::mpsc::UnboundedSender<removent_core::ipc::IpcRequest>>>>;

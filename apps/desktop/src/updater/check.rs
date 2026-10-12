use super::*;

/// One update check (manual button or the 30s/24h scheduler). Runs on a
/// blocking thread. Scheduled checks never clobber a state the user is acting
/// on (an available/staged update survives the next 24h tick).
pub fn run_check(
    manual: bool,
    shared: Arc<Mutex<UpdateShared>>,
    events: std::sync::mpsc::Sender<UiEvent>,
) {
    run_check_with_fetch(manual, shared, events, fetch_and_validate);
}

fn run_check_with_fetch(
    manual: bool,
    shared: Arc<Mutex<UpdateShared>>,
    events: std::sync::mpsc::Sender<UiEvent>,
    fetch: impl FnOnce(&UpdatePolicy) -> Result<Option<UpdateManifest>, String>,
) {
    let policy = {
        let mut state = shared.lock().unwrap();
        let status = &state.status;
        // Never interrupt in-flight work. A staged update (ReadyToInstall)
        // survives even a manual re-check — overwriting it would orphan the
        // staged bundle and force a re-download; scheduled checks also leave
        // an available update untouched (the user may be about to act on it).
        if status.is_busy()
            || matches!(status, UpdateStatus::ReadyToInstall { .. })
            || (!manual && matches!(status, UpdateStatus::Available { .. }))
        {
            return;
        }
        state.status = UpdateStatus::Checking;
        state.generation = state.generation.wrapping_add(1);
        state.manifest = None;
        let _ = events.send(UiEvent::UpdateStatus(UpdateStatus::Checking));
        state.policy.clone()
    };
    let result = fetch(&policy);
    let mut state = shared.lock().unwrap();
    // Record completed attempts, including failures. Skipped/busy checks above
    // never overwrite the time. Persist before notifying the UI.
    state.last_checked_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|time| time.as_secs());
    if let (Some(path), Some(last)) = (&state.history_path, state.last_checked_at)
        && let Err(error) = history::save(path, last)
    {
        tracing::warn!(%error, "could not save update check time");
    }
    state.status = match result {
        Ok(Some(m)) => {
            let available = UpdateStatus::Available {
                version: m.version.clone(),
                notes: m.notes.clone(),
            };
            state.manifest = Some(m);
            available
        }
        Ok(None) => UpdateStatus::UpToDate { manual },
        Err(reason) => UpdateStatus::Failed(reason),
    };
    let _ = events.send(UiEvent::UpdateStatus(state.status.clone()));
}

/// Fetch and validate the manifest; Ok(Some) = a newer compatible version exists.
pub(super) fn fetch_and_validate(policy: &UpdatePolicy) -> Result<Option<UpdateManifest>, String> {
    fetch_update(
        policy,
        env!("CARGO_PKG_VERSION"),
        &release_verifying_key().ok_or_else(|| t!("update.err.signature").to_string())?,
        curl_get,
    )
}

#[derive(Debug, Deserialize)]
pub(super) struct GithubRelease {
    pub(super) tag_name: String,
    pub(super) draft: bool,
    pub(super) prerelease: bool,
    pub(super) assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GithubAsset {
    pub(super) name: String,
    pub(super) browser_download_url: String,
}

/// Compare versions, never release timestamps or list order. The beta channel
/// includes stable releases so a final release supersedes its earlier betas.
pub(super) fn release_candidate(
    releases: &[GithubRelease],
    local: &str,
) -> Option<(semver::Version, String)> {
    releases
        .iter()
        .filter_map(|r| {
            let version = parse_version(&r.tag_name)?;
            if r.draft
                || r.prerelease == version.pre.is_empty()
                || !eligible_upgrade(UpdateChannel::Beta, &r.tag_name, local)
            {
                return None;
            }
            let asset = r.assets.iter().find(|a| a.name == "latest.json")?;
            // Release discovery is unsigned. Only official immutable assets can
            // supply the signed manifest, whose version is also matched to the tag.
            let expected = format!(
                "https://github.com/backrunner/removent/releases/download/{}/latest.json",
                r.tag_name
            );
            (asset.browser_download_url == expected).then_some((version, expected))
        })
        .max_by(|a, b| a.0.cmp_precedence(&b.0))
}

pub(super) fn fetch_update(
    policy: &UpdatePolicy,
    local: &str,
    key: &VerifyingKey,
    mut fetch: impl FnMut(&str) -> Result<String, String>,
) -> Result<Option<UpdateManifest>, String> {
    let (endpoint, expected_version) = if !policy.endpoint.is_empty() {
        (policy.endpoint.clone(), None)
    } else if policy.channel == UpdateChannel::Stable {
        (DEFAULT_MANIFEST_URL.into(), None)
    } else {
        let mut candidate: Option<(semver::Version, String)> = None;
        // Paginate instead of assuming GitHub's publish order is SemVer order.
        // Bound API work and fail closed if the history exceeds this limit.
        for page in 1..=10 {
            let body = fetch(&format!("{RELEASES_API}?per_page=100&page={page}"))?;
            let releases: Vec<GithubRelease> = serde_json::from_str(&body)
                .map_err(|e| t!("update.err.bad_manifest", err = e.to_string()).to_string())?;
            if let Some(next) = release_candidate(&releases, local)
                && candidate
                    .as_ref()
                    .is_none_or(|old| next.0.cmp_precedence(&old.0).is_gt())
            {
                candidate = Some(next);
            }
            if releases.len() < 100 {
                break;
            }
            if page == 10 {
                return Err(t!(
                    "update.err.bad_manifest",
                    err = "release history exceeds lookup limit"
                )
                .to_string());
            }
        }
        let Some((version, url)) = candidate else {
            return Ok(None);
        };
        (url, Some(version))
    };
    let body = fetch(&endpoint)?;
    let m: UpdateManifest = serde_json::from_str(&body)
        .map_err(|e| t!("update.err.bad_manifest", err = e.to_string()).to_string())?;
    // Signature before anything else: a tampered or unsigned manifest is
    // rejected before any download is even considered (release.md §3.2).
    if !verify_manifest_signature(&m, key) {
        return Err(t!("update.err.signature").to_string());
    }
    if parse_version(&m.version).is_none()
        || !m.url.starts_with("https://")
        || m.sha256.len() != 64
        || hex::decode(&m.sha256).is_err()
    {
        return Err(t!(
            "update.err.bad_manifest",
            err = "invalid version, URL or digest"
        )
        .to_string());
    }
    if expected_version.is_some_and(|v| Some(v) != parse_version(&m.version)) {
        return Err(t!(
            "update.err.bad_manifest",
            err = "release tag and signed version differ"
        )
        .to_string());
    }
    if !eligible_upgrade(policy.channel, &m.version, local) {
        return Ok(None);
    }
    // Protocol floor above ours: installing would break interconnection — tell
    // the user to upgrade both ends together instead of installing.
    if m.min_compatible_proto > removent_proto::PROTO_VERSION {
        return Err(t!("update.err.proto_mismatch").to_string());
    }
    Ok(Some(m))
}

// ---- download + verify ----

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_record_completion_and_preserve_manual_origin_across_restart() {
        let directory = tempfile::tempdir().unwrap();
        let paths = DataPaths {
            root: directory.path().into(),
        };
        let settings = Settings::default();
        let shared = UpdateShared::with_history(&settings, &paths);
        for manual in [false, true, false] {
            let previous = shared.lock().unwrap().last_checked_at;
            let (tx, rx) = std::sync::mpsc::channel();
            run_check_with_fetch(manual, shared.clone(), tx, |_| {
                assert_eq!(shared.lock().unwrap().last_checked_at, previous);
                Ok(None)
            });
            assert!(matches!(
                rx.try_recv().unwrap(),
                UiEvent::UpdateStatus(UpdateStatus::Checking)
            ));
            assert!(
                matches!(rx.try_recv().unwrap(), UiEvent::UpdateStatus(UpdateStatus::UpToDate { manual: origin }) if origin == manual)
            );
            let last = shared.lock().unwrap().last_checked_at.unwrap();
            let restored = UpdateShared::with_history(&settings, &paths);
            assert_eq!(restored.lock().unwrap().last_checked_at, Some(last));
            // The historical time survives restart; a manual result does not.
            assert_eq!(restored.lock().unwrap().status, UpdateStatus::Idle);
        }
    }

    #[test]
    fn automatic_candidates_and_failed_attempts_keep_their_result_and_time() {
        let directory = tempfile::tempdir().unwrap();
        let paths = DataPaths {
            root: directory.path().into(),
        };
        let settings = Settings::default();
        let shared = UpdateShared::with_history(&settings, &paths);
        let (tx, _) = std::sync::mpsc::channel();
        run_check_with_fetch(false, shared.clone(), tx, |_| {
            Ok(Some(UpdateManifest {
                version: "9.0.0".into(),
                url: "https://example.invalid/update".into(),
                sha256: String::new(),
                signature: String::new(),
                notes: "Release notes".into(),
                pub_date: String::new(),
                min_compatible_proto: 1,
            }))
        });
        assert!(matches!(
            shared.lock().unwrap().status,
            UpdateStatus::Available { .. }
        ));
        let (tx, _) = std::sync::mpsc::channel();
        run_check_with_fetch(true, shared.clone(), tx, |_| Err("Offline".into()));
        assert_eq!(
            shared.lock().unwrap().status,
            UpdateStatus::Failed("Offline".into())
        );
        assert!(
            UpdateShared::with_history(&settings, &paths)
                .lock()
                .unwrap()
                .last_checked_at
                .is_some()
        );
    }

    #[test]
    fn skipped_checks_leave_history_untouched_and_a_save_failure_keeps_the_result() {
        let directory = tempfile::tempdir().unwrap();
        let paths = DataPaths {
            root: directory.path().into(),
        };
        let settings = Settings::default();
        history::save(&paths.root.join("update-check.json"), 1234567890).unwrap();
        let shared = UpdateShared::with_history(&settings, &paths);
        for (manual, status) in [
            (true, UpdateStatus::Checking),
            (
                true,
                UpdateStatus::ReadyToInstall {
                    version: "9.0.0".into(),
                },
            ),
            (
                false,
                UpdateStatus::Available {
                    version: "9.0.0".into(),
                    notes: String::new(),
                },
            ),
        ] {
            shared.lock().unwrap().status = status.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            run_check_with_fetch(manual, shared.clone(), tx, |_| {
                panic!("skipped check must not fetch")
            });
            assert_eq!(shared.lock().unwrap().status, status);
            assert_eq!(shared.lock().unwrap().last_checked_at, Some(1234567890));
            assert!(rx.try_recv().is_err());
        }
        std::fs::remove_file(paths.root.join("update-check.json")).unwrap();
        std::fs::create_dir(paths.root.join("update-check.json")).unwrap();
        shared.lock().unwrap().status = UpdateStatus::Idle;
        let (tx, _) = std::sync::mpsc::channel();
        run_check_with_fetch(true, shared.clone(), tx, |_| Ok(None));
        assert_eq!(
            shared.lock().unwrap().status,
            UpdateStatus::UpToDate { manual: true }
        );
        assert!(shared.lock().unwrap().last_checked_at.unwrap() > 1234567890);
    }
}

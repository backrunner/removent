use super::*;

/// One update check (manual button or the 30s/24h scheduler). Runs on a
/// blocking thread. Scheduled checks never clobber a state the user is acting
/// on (an available/staged update survives the next 24h tick).
pub fn run_check(
    manual: bool,
    shared: Arc<Mutex<UpdateShared>>,
    events: std::sync::mpsc::Sender<UiEvent>,
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
    match fetch_and_validate(&policy) {
        Ok(Some(m)) => {
            let available = UpdateStatus::Available {
                version: m.version.clone(),
                notes: m.notes.clone(),
            };
            shared.lock().unwrap().manifest = Some(m);
            set_status(&shared, &events, available);
        }
        Ok(None) => set_status(&shared, &events, UpdateStatus::UpToDate),
        Err(reason) => set_status(&shared, &events, UpdateStatus::Failed(reason)),
    }
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

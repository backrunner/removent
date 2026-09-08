//! Client auto-update (release.md §3): GitHub Releases manifest check, signed
//! download, triple verification (sha256 + ed25519 + codesign), atomic .app swap
//! and relaunch.
//!
//! All network I/O shells out to `curl` (always present on macOS; the project
//! already shells out for `open`/`pgrep`) so no HTTP dependency is added.
//! Blocking work runs on the tokio blocking pool; the UI follows progress via
//! `UiEvent::UpdateStatus`.

use crate::engine::UiEvent;
use ed25519_dalek::VerifyingKey;
use removent_core::{DataPaths, Settings};
use rust_i18n::t;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Stable manifest URL (the `latest.json` asset of the newest GitHub Release).
pub const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/backrunner/removent/releases/latest/download/latest.json";

/// GitHub's latest endpoint excludes prereleases. Beta builds discover immutable
/// per-release manifests through the releases API, including a later stable release.
pub const BETA_RELEASES_URL: &str =
    "https://api.github.com/repos/backrunner/removent/releases?per_page=100";

/// Release-signing public key (Ed25519, hex). The private key only lives in the
/// CI secrets of the release pipeline (release.md §2).
const RELEASE_PUBLIC_KEY_HEX: &str =
    "2b51160721f9eee916944e4603e6540c628247e5284a79b099392f086a3def0c";

/// Developer ID team used for the official distribution. Never accept ad-hoc
/// signatures or another developer's valid signature as an official update.
const EXPECTED_TEAM_ID: &str = "PB8H83VL3Z";

/// Update manifest contract (`latest.json`, shared with the release pipeline —
/// field names must not change).
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub notes: String,
    /// Release timestamp (contract field; not displayed in the UI yet).
    #[serde(default)]
    #[allow(dead_code)]
    pub pub_date: String,
    pub min_compatible_proto: u16,
    pub signature: String,
}

/// Update state machine snapshot (release.md §3.2):
/// Idle → Checking → Downloading → Verifying → ReadyToInstall → Swapping →
/// Relaunching; any failure lands in `Failed` with a user-facing reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    Idle,
    Checking,
    /// Check finished, no newer version.
    UpToDate,
    /// A newer, signature-valid version exists (prompt only, never forced).
    Available {
        version: String,
        notes: String,
    },
    Downloading {
        version: String,
    },
    Verifying {
        version: String,
    },
    /// Downloaded and triple-verified; waiting to swap (a running session
    /// defers the install until it ends).
    ReadyToInstall {
        version: String,
    },
    Swapping {
        version: String,
    },
    Relaunching,
    Failed(String),
}

impl UpdateStatus {
    /// True while a check/download/verify/swap is in flight (buttons disabled).
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Checking
                | Self::Downloading { .. }
                | Self::Verifying { .. }
                | Self::Swapping { .. }
                | Self::Relaunching
        )
    }
}

/// Shared updater state (engine ↔ worker threads).
pub struct UpdateShared {
    pub status: UpdateStatus,
    /// Manifest of the available update (Some while Available or later).
    manifest: Option<UpdateManifest>,
    /// Verified, unpacked .app staged in the update cache, ready for the swap.
    staged_app: Option<PathBuf>,
}

impl UpdateShared {
    pub fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            status: UpdateStatus::Idle,
            manifest: None,
            staged_app: None,
        }))
    }
}

/// Manifest endpoint: the settings override wins (enterprise mirrors), otherwise
/// choose the GitHub channel from the compiled-in SemVer.
pub fn manifest_endpoint(settings: &Settings) -> String {
    let custom = settings.update_endpoint.trim();
    if custom.is_empty() {
        if parse_version(env!("CARGO_PKG_VERSION")).is_some_and(|v| !v.pre.is_empty()) {
            BETA_RELEASES_URL.to_string()
        } else {
            DEFAULT_MANIFEST_URL.to_string()
        }
    } else {
        custom.to_string()
    }
}

fn set_status(
    shared: &Arc<Mutex<UpdateShared>>,
    events: &std::sync::mpsc::Sender<UiEvent>,
    status: UpdateStatus,
) {
    shared.lock().unwrap().status = status.clone();
    let _ = events.send(UiEvent::UpdateStatus(status));
}

// ---- pure logic (unit-tested) ----

/// Strict SemVer including beta identifiers (beta.10 sorts after beta.2).
pub fn parse_version(s: &str) -> Option<semver::Version> {
    semver::Version::parse(s.trim().strip_prefix('v').unwrap_or(s.trim())).ok()
}

pub fn is_newer(remote: &str, local: &str) -> bool {
    match (parse_version(remote), parse_version(local)) {
        (Some(r), Some(l)) => r.cmp_precedence(&l).is_gt(),
        _ => false,
    }
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

fn beta_manifest_url(body: &str) -> Result<Option<String>, String> {
    let releases: Vec<GithubRelease> = serde_json::from_str(body)
        .map_err(|e| t!("update.err.bad_manifest", err = e.to_string()).to_string())?;
    Ok(releases
        .into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| {
            let version = parse_version(&r.tag_name)?;
            // Beta users can graduate to stable; never opt them into alpha/nightly.
            if !version.pre.is_empty() && !version.pre.as_str().starts_with("beta.") {
                return None;
            }
            let asset = r.assets.into_iter().find(|a| a.name == "latest.json")?;
            Some((version, asset.browser_download_url))
        })
        .max_by(|a, b| a.0.cmp_precedence(&b.0))
        .map(|(_, url)| url))
}

/// Ed25519 signature payload: `"{version}\n{url}\n{sha256}\n{min_compatible_proto}"`
/// (four lines, no trailing newline — contract with the release pipeline).
pub fn signature_payload(m: &UpdateManifest) -> Vec<u8> {
    format!(
        "{}\n{}\n{}\n{}",
        m.version, m.url, m.sha256, m.min_compatible_proto
    )
    .into_bytes()
}

/// Verify the manifest's detached Ed25519 signature (64-byte hex) against the
/// release public key.
pub fn verify_manifest_signature(m: &UpdateManifest, key: &VerifyingKey) -> bool {
    let sig_bytes = match hex::decode(m.signature.trim()) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let Ok(sig_arr) = <[u8; 64]>::try_from(sig_bytes.as_slice()) else {
        return false;
    };
    let sig = ed25519_dalek::Signature::from_bytes(&sig_arr);
    key.verify_strict(&signature_payload(m), &sig).is_ok()
}

/// The compiled-in release verification key (None only on a corrupted constant).
pub fn release_verifying_key() -> Option<VerifyingKey> {
    let bytes = hex::decode(RELEASE_PUBLIC_KEY_HEX).ok()?;
    let arr = <[u8; 32]>::try_from(bytes.as_slice()).ok()?;
    VerifyingKey::from_bytes(&arr).ok()
}

// ---- check ----

/// One update check (manual button or the 30s/24h scheduler). Runs on a
/// blocking thread. Scheduled checks never clobber a state the user is acting
/// on (an available/staged update survives the next 24h tick).
pub fn run_check(
    endpoint: String,
    manual: bool,
    shared: Arc<Mutex<UpdateShared>>,
    events: std::sync::mpsc::Sender<UiEvent>,
) {
    {
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
        let _ = events.send(UiEvent::UpdateStatus(UpdateStatus::Checking));
    }
    match fetch_and_validate(&endpoint) {
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
fn fetch_and_validate(endpoint: &str) -> Result<Option<UpdateManifest>, String> {
    let body = curl_get(endpoint)?;
    let body = if endpoint == BETA_RELEASES_URL {
        let Some(url) = beta_manifest_url(&body)? else {
            return Ok(None);
        };
        curl_get(&url)?
    } else {
        body
    };
    let m: UpdateManifest = serde_json::from_str(&body)
        .map_err(|e| t!("update.err.bad_manifest", err = e.to_string()).to_string())?;
    // Signature before anything else: a tampered or unsigned manifest is
    // rejected before any download is even considered (release.md §3.2).
    let key = release_verifying_key().ok_or_else(|| t!("update.err.signature").to_string())?;
    if !verify_manifest_signature(&m, &key) {
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
    // Protocol floor above ours: installing would break interconnection — tell
    // the user to upgrade both ends together instead of installing.
    if m.min_compatible_proto > removent_proto::PROTO_VERSION {
        return Err(t!("update.err.proto_mismatch").to_string());
    }
    if is_newer(&m.version, env!("CARGO_PKG_VERSION")) {
        Ok(Some(m))
    } else {
        Ok(None)
    }
}

// ---- download + verify ----

/// Download the available update and run the triple verification. Runs on a
/// blocking thread; on success the state becomes ReadyToInstall.
pub fn run_download(
    paths: DataPaths,
    shared: Arc<Mutex<UpdateShared>>,
    events: std::sync::mpsc::Sender<UiEvent>,
) {
    // Concurrency guard (double-click on "Download and Install"): claim the
    // Available → Downloading transition inside one lock; a second click sees
    // a non-Available state and returns instead of racing the same .part file.
    let m = {
        let mut s = shared.lock().unwrap();
        let UpdateStatus::Available { version, .. } = &s.status else {
            return;
        };
        let Some(m) = s.manifest.clone() else {
            return;
        };
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

fn download_and_stage(
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
    let version = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :RemoventReleaseVersion"])
        .arg(app.join("Contents/Info.plist"))
        .output()
        .map_err(|e| t!("update.err.unpack", err = e.to_string()).to_string())?;
    if !version.status.success() || String::from_utf8_lossy(&version.stdout).trim() != m.version {
        return Err(t!(
            "update.err.unpack",
            err = "bundle version does not match signed manifest"
        )
        .to_string());
    }
    Ok(app)
}

// ---- swap + relaunch ----

/// daemon IPC request outlet (shared with the engine); used to stop the old
/// daemon before relaunching after an update.
type DaemonReq =
    Arc<Mutex<Option<tokio::sync::mpsc::UnboundedSender<removent_core::ipc::IpcRequest>>>>;

/// Swap the staged .app into place and relaunch. Runs on a blocking thread;
/// the process exits on success.
pub fn run_install(
    shared: Arc<Mutex<UpdateShared>>,
    events: std::sync::mpsc::Sender<UiEvent>,
    daemon_req: DaemonReq,
) {
    // Concurrency guard (double-click on "Install"): claim the ReadyToInstall →
    // Swapping transition inside one lock; a second call returns instead of
    // racing swap_bundle (whose backup cleanup would delete the first swap's
    // rollback copy).
    let staged = {
        let mut s = shared.lock().unwrap();
        let UpdateStatus::ReadyToInstall { version } = &s.status else {
            return;
        };
        let Some(staged) = s.staged_app.clone() else {
            return;
        };
        s.status = UpdateStatus::Swapping {
            version: version.clone(),
        };
        let _ = events.send(UiEvent::UpdateStatus(s.status.clone()));
        staged
    };
    match swap_bundle(&staged) {
        Ok(bundle) => {
            // Stop the old daemon before relaunching: it ships inside the
            // bundle, and a daemon spawned by the app (not launchd) would
            // otherwise keep running the old version while the new app
            // connects to it. launchd restarts it via the kickstart in
            // relaunch; otherwise the new app re-spawns it on demand.
            if let Some(tx) = daemon_req.lock().unwrap().as_ref() {
                let _ = tx.send(removent_core::ipc::IpcRequest::Shutdown);
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
            set_status(&shared, &events, UpdateStatus::Relaunching);
            if let Err(error) = relaunch(&bundle) {
                let backup = bundle.with_extension("app.old");
                let failed = bundle.with_extension("app.failed");
                let restored = std::fs::rename(&bundle, &failed)
                    .and_then(|()| std::fs::rename(&backup, &bundle));
                let reason = if restored.is_ok() {
                    let _ = std::fs::remove_dir_all(failed);
                    // The replacement daemon may already have started. Put
                    // the restored bundle's server back under the same job.
                    #[cfg(target_os = "macos")]
                    if let Ok(service) = removent_core::service::Service::new(
                        removent_core::DataPaths::resolve(),
                        bundle.join("Contents/MacOS/removentd"),
                    ) {
                        let _ = service.restart();
                    }
                    t!("update.err.swap", err = error.to_string()).to_string()
                } else {
                    t!(
                        "update.err.swap_rollback",
                        err = error.to_string(),
                        backup = backup.display().to_string()
                    )
                    .to_string()
                };
                set_status(&shared, &events, UpdateStatus::Failed(reason));
            }
        }
        Err(reason) => set_status(&shared, &events, UpdateStatus::Failed(reason)),
    }
}

/// mv the running bundle aside (`Removent.app.old`) and move the staged bundle
/// in, rolling back when the new bundle cannot be moved into place.
fn swap_bundle(staged_app: &Path) -> Result<PathBuf, String> {
    let Some(bundle) = current_bundle() else {
        // Not running from a .app (cargo run / target dir): refuse to install.
        return Err(t!("update.err.not_in_bundle").to_string());
    };
    replace_bundle(staged_app, &bundle)?;
    Ok(bundle)
}

/// Stage on the destination volume before moving the running app. A failed
/// cross-volume copy must never leave a partial new bundle blocking rollback.
fn replace_bundle(staged_app: &Path, bundle: &Path) -> Result<(), String> {
    let incoming = bundle.with_extension("app.incoming");
    let backup = bundle.with_extension("app.old");
    let fail = |e: std::io::Error| t!("update.err.swap", err = e.to_string()).to_string();
    if incoming.exists() {
        std::fs::remove_dir_all(&incoming).map_err(fail)?;
    }
    if let Err(error) = move_into_place(staged_app, &incoming) {
        let _ = std::fs::remove_dir_all(&incoming);
        return Err(fail(error));
    }
    // Copying has completed. Only the two same-volume renames happen while
    // the original application path is temporarily unavailable.
    if backup.exists() {
        std::fs::remove_dir_all(&backup).map_err(fail)?;
    }
    std::fs::rename(bundle, &backup).map_err(fail)?;
    if let Err(error) = std::fs::rename(&incoming, bundle) {
        if std::fs::rename(&backup, bundle).is_err() {
            return Err(t!(
                "update.err.swap_rollback",
                err = error.to_string(),
                backup = backup.display().to_string()
            )
            .to_string());
        }
        return Err(fail(error));
    }
    Ok(())
}

fn move_into_place(staged: &Path, bundle: &Path) -> std::io::Result<()> {
    match std::fs::rename(staged, bundle) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            let copied = Command::new("/usr/bin/ditto")
                .arg(staged)
                .arg(bundle)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()?;
            if !copied.success() {
                return Err(std::io::Error::other(format!("ditto exit {copied}")));
            }
            // Failure to remove the cache must not invalidate a successful copy.
            let _ = std::fs::remove_dir_all(staged);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Restart the daemon (it ships inside the bundle; best-effort — it may not be
/// loaded at all), open the new app and exit only if Launch Services accepts it.
fn relaunch(bundle: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    removent_core::service::Service::new(
        removent_core::DataPaths::resolve(),
        bundle.join("Contents/MacOS/removentd"),
    )
    .and_then(|service| service.restart())
    .map_err(std::io::Error::other)?;
    let opened = Command::new("/usr/bin/open")
        .arg("-n")
        .arg(bundle)
        .status()?;
    if !opened.success() {
        return Err(std::io::Error::other(format!("open exit {opened}")));
    }
    std::process::exit(0);
}

/// Locate the running .app bundle by walking up from the current executable
/// (`<bundle>.app/Contents/MacOS/removent`); None outside a bundle (dev runs).
fn current_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(|p| p.to_path_buf())
}

/// Remove the `Removent.app.old` backup left by a previous update (called once
/// at startup — reaching this code means the new version launched fine).
pub fn cleanup_stale_backup() {
    std::thread::Builder::new()
        .name("update-cleanup".into())
        .spawn(|| {
            let Some(bundle) = current_bundle() else {
                return;
            };
            let backup = bundle.with_extension("app.old");
            if backup.exists() {
                let _ = std::fs::remove_dir_all(&backup);
            }
        })
        .expect("spawn update cleanup");
}

// ---- shell helpers ----

#[derive(Debug)]
struct FetchError {
    exit_code: Option<i32>,
    http_status: Option<u16>,
    detail: String,
}

impl FetchError {
    fn is_transient(&self) -> bool {
        // Retry interrupted transfers as well as temporary server failures.
        // A missing asset, denied request or invalid certificate needs a fix,
        // not more requests (GitHub also reports API rate limits with 403).
        matches!(
            self.exit_code,
            Some(5 | 6 | 7 | 18 | 28 | 52 | 55 | 56 | 92)
        ) || (self.exit_code == Some(22)
            && matches!(self.http_status, Some(408 | 429 | 500 | 502 | 503 | 504)))
    }

    fn user_message(&self) -> String {
        let reason = match self.http_status {
            Some(403) => t!("update.err.http_forbidden").to_string(),
            Some(404) => t!("update.err.http_not_found").to_string(),
            Some(429) => t!("update.err.http_rate_limit").to_string(),
            Some(status) if status >= 400 => format!("HTTP {status}"),
            _ => match self.exit_code {
                Some(5 | 6) => t!("update.err.dns").to_string(),
                Some(7) => t!("update.err.connect").to_string(),
                Some(28) => t!("update.err.timeout").to_string(),
                Some(35 | 60) => t!("update.err.tls").to_string(),
                _ => self.detail.clone(),
            },
        };
        t!("update.err.fetch", err = reason).to_string()
    }
}

fn curl_get(url: &str) -> Result<String, String> {
    fetch_with_retry(url, Duration::from_secs(30), curl_get_once)
}

fn fetch_with_retry(
    url: &str,
    timeout: Duration,
    mut fetch: impl FnMut(&str, Duration) -> Result<Vec<u8>, FetchError>,
) -> Result<String, String> {
    let deadline = Instant::now() + timeout;
    for attempt in 0..3 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match fetch(url, remaining) {
            Ok(body) => {
                return String::from_utf8(body)
                    .map_err(|e| t!("update.err.bad_manifest", err = e.to_string()).to_string());
            }
            Err(error) => {
                let delay = Duration::from_secs(1 << attempt);
                let retry = attempt < 2
                    && error.is_transient()
                    && deadline.saturating_duration_since(Instant::now()) > delay;
                // Avoid recording mirror URLs or raw curl stderr: either can
                // contain credentials. Keep enough context to diagnose failures.
                tracing::warn!(
                    source = if url == BETA_RELEASES_URL {
                        "beta releases"
                    } else {
                        "manifest"
                    },
                    attempt = attempt + 1,
                    exit_code = error.exit_code,
                    http_status = error.http_status,
                    retry,
                    "update request failed"
                );
                if !retry {
                    return Err(error.user_message());
                }
                std::thread::sleep(delay);
            }
        }
    }
    unreachable!("the final attempt always returns")
}

fn curl_get_once(url: &str, timeout: Duration) -> Result<Vec<u8>, FetchError> {
    let out = Command::new("/usr/bin/curl")
        .args([
            "-fsSL",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "10",
            "--max-time",
            &format!("{:.3}", timeout.as_secs_f64().max(0.001)),
            "--max-filesize",
            "2097152",
            "--user-agent",
            "Removent-Updater",
            // Keep the status separate from the body, including for errors.
            "--write-out",
            "%{stderr}\n%{http_code}",
            "--url",
            url,
        ])
        .output()
        .map_err(|e| FetchError {
            exit_code: None,
            http_status: None,
            detail: e.to_string(),
        })?;
    decode_fetch_output(out)
}

fn decode_fetch_output(out: std::process::Output) -> Result<Vec<u8>, FetchError> {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let (detail, status) = stderr.rsplit_once('\n').unwrap_or((&stderr, ""));
    let http_status = status.trim().parse::<u16>().ok().filter(|s| *s != 0);
    if !out.status.success() {
        let detail = detail.trim().chars().take(512).collect::<String>();
        return Err(FetchError {
            exit_code: out.status.code(),
            http_status,
            detail: if detail.is_empty() {
                format!("curl {}", out.status)
            } else {
                detail
            },
        });
    }
    Ok(out.stdout)
}

/// Download to a `.part` file; `resume` continues a partial download (`-C -`).
fn curl_download(url: &str, part: &Path, resume: bool) -> bool {
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

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| t!("update.err.download", err = e.to_string()).to_string())?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .map_err(|e| t!("update.err.download", err = e.to_string()).to_string())?;
    Ok(hex::encode(hasher.finalize()))
}

/// The release zip holds a single top-level `Removent.app`.
fn find_app_bundle(stage: &Path) -> Option<PathBuf> {
    let app = stage.join("Removent.app");
    let metadata = std::fs::symlink_metadata(&app).ok()?;
    (metadata.is_dir() && !metadata.file_type().is_symlink()).then_some(app)
}

fn release_codesign_requirement() -> String {
    // codesign treats -R as a filename unless the expression starts with '='.
    format!(
        "=anchor apple generic and identifier \"io.removent.app\" and certificate leaf[subject.OU] = \"{EXPECTED_TEAM_ID}\" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists"
    )
}

fn codesign_verify(app: &Path) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use std::os::unix::process::ExitStatusExt;

    fn fetch_output(code: i32, body: &[u8], stderr: &str) -> std::process::Output {
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: body.to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn fetch_preserves_manifest_body_without_status_metadata() {
        let body = b"{\"version\":\"0.1.0-beta.4\"}\n";
        assert_eq!(
            decode_fetch_output(fetch_output(0, body, "\n200")).unwrap(),
            body
        );
    }

    #[test]
    fn fetch_distinguishes_http_failures_with_the_same_curl_exit_code() {
        for (status, retry) in [(403, false), (404, false), (429, true), (503, true)] {
            let error = decode_fetch_output(fetch_output(
                22,
                b"",
                &format!("curl: (22) The requested URL returned error: {status}\n\n{status}"),
            ))
            .unwrap_err();
            assert_eq!(error.http_status, Some(status));
            assert_eq!(error.is_transient(), retry);
            assert!(error.user_message().contains(&format!("HTTP {status}")));
        }
    }

    #[test]
    fn fetch_retries_interrupted_transfers_but_rejects_certificate_failures() {
        let interrupted = decode_fetch_output(fetch_output(
            18,
            b"{\"version\":",
            "curl: (18) transfer closed with outstanding read data remaining\n\n200",
        ))
        .unwrap_err();
        assert!(interrupted.is_transient());
        assert_eq!(interrupted.http_status, Some(200));
        let timeout =
            decode_fetch_output(fetch_output(28, b"", "curl: (28) timeout\n\n000")).unwrap_err();
        assert!(timeout.is_transient());
        assert_eq!(timeout.http_status, None);
        let certificate = decode_fetch_output(fetch_output(
            60,
            b"",
            "curl: (60) SSL certificate problem\n\n000",
        ))
        .unwrap_err();
        assert!(!certificate.is_transient());
    }

    #[test]
    fn retry_discards_partial_body_and_uses_the_remaining_time_budget() {
        let mut budgets = Vec::new();
        let body = fetch_with_retry(
            "https://example.com",
            Duration::from_secs(30),
            |_, budget| {
                budgets.push(budget);
                decode_fetch_output(if budgets.len() == 1 {
                    fetch_output(18, b"{\"version\":", "curl: (18) interrupted\n\n200")
                } else {
                    fetch_output(0, b"{\"version\":\"0.1.0\"}", "\n200")
                })
            },
        )
        .unwrap();
        assert_eq!(body, "{\"version\":\"0.1.0\"}");
        assert_eq!(budgets.len(), 2);
        assert!(budgets[1] < budgets[0]);
        assert!(budgets[0] <= Duration::from_secs(30));
    }

    #[test]
    fn retry_stops_on_permanent_failure_or_exhausted_budget() {
        for (status, budget) in [
            (404, Duration::from_secs(30)),
            (503, Duration::from_millis(20)),
        ] {
            let mut attempts = 0;
            let error = fetch_with_retry("https://example.com", budget, |_, _| {
                attempts += 1;
                decode_fetch_output(fetch_output(22, b"", &format!("\n{status}")))
            })
            .unwrap_err();
            assert_eq!(attempts, 1);
            assert!(error.contains(&format!("HTTP {status}")));
        }
    }

    #[test]
    #[ignore = "requires access to the public GitHub release feed"]
    fn official_beta_feed_fetches_a_valid_signed_manifest() {
        // Exercise the same discovery, HTTPS requests and signature validation
        // as the app without downloading or installing an update.
        let feed = curl_get(BETA_RELEASES_URL).unwrap();
        let url = beta_manifest_url(&feed)
            .unwrap()
            .expect("the official feed must contain a release manifest");
        fetch_and_validate(&url).unwrap();
    }

    #[test]
    fn release_requirement_is_accepted_as_inline_source() {
        let result = Command::new("/usr/bin/csreq")
            .args(["-r", &release_codesign_requirement(), "-t"])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    fn manifest() -> UpdateManifest {
        UpdateManifest {
            version: "0.1.1".into(),
            url: "https://example.com/Removent-0.1.1-macos-arm64.zip".into(),
            sha256: "ab12".into(),
            notes: "notes".into(),
            pub_date: "2026-08-23T00:00:00Z".into(),
            min_compatible_proto: 1,
            signature: String::new(),
        }
    }

    #[test]
    fn version_parse_and_compare() {
        assert_eq!(parse_version("1.2.3"), Some(semver::Version::new(1, 2, 3)));
        assert_eq!(
            parse_version("v0.10.2"),
            Some(semver::Version::new(0, 10, 2))
        );
        assert_eq!(
            parse_version(" 1.0.0 "),
            Some(semver::Version::new(1, 0, 0))
        );
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version("a.b.c"), None);

        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.1"));
        assert!(!is_newer("garbage", "0.1.0"));
        assert!(is_newer("0.1.0-beta.2", "0.1.0-beta.1"));
        assert!(is_newer("0.1.0-beta.10", "0.1.0-beta.2"));
        assert!(is_newer("0.1.0", "0.1.0-beta.10"));
        assert!(!is_newer("0.1.0-beta.10", "0.1.0"));
        assert!(!is_newer("0.1.0+build2", "0.1.0+build1"));
    }

    #[test]
    fn beta_feed_selects_semver_and_excludes_drafts_and_missing_assets() {
        let release = |tag: &str, draft: bool, asset: &str| {
            serde_json::json!({
                "tag_name": tag, "draft": draft,
                "assets": [{"name": asset, "browser_download_url": format!("https://example.com/{tag}")}]
            })
        };
        let mut feed = vec![
            release("v0.1.0-beta.2", false, "latest.json"),
            release("v0.1.0-beta.10", false, "latest.json"),
            release("v2.0.0", true, "latest.json"),
            release("v3.0.0-alpha.1", false, "latest.json"),
            release("v4.0.0", false, "other.json"),
        ];
        assert_eq!(
            beta_manifest_url(&serde_json::to_string(&feed).unwrap()).unwrap(),
            Some("https://example.com/v0.1.0-beta.10".into())
        );
        feed.push(release("v0.1.0", false, "latest.json"));
        assert_eq!(
            beta_manifest_url(&serde_json::to_string(&feed).unwrap()).unwrap(),
            Some("https://example.com/v0.1.0".into())
        );
        assert_eq!(beta_manifest_url("[]").unwrap(), None);
        assert!(beta_manifest_url("{}").is_err());
    }

    #[test]
    fn staging_failure_preserves_installed_bundle_and_backup() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Removent.app");
        let backup = bundle.with_extension("app.old");
        std::fs::create_dir(&bundle).unwrap();
        std::fs::write(bundle.join("version"), "current").unwrap();
        std::fs::create_dir(&backup).unwrap();
        assert!(replace_bundle(&dir.path().join("missing.app"), &bundle).is_err());
        assert_eq!(
            std::fs::read_to_string(bundle.join("version")).unwrap(),
            "current"
        );
        assert!(backup.exists());
    }

    #[test]
    fn successful_swap_keeps_previous_version_for_rollback() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Removent.app");
        let staged = dir.path().join("stage.app");
        for (path, version) in [(&bundle, "old"), (&staged, "new")] {
            std::fs::create_dir(path).unwrap();
            std::fs::write(path.join("version"), version).unwrap();
        }
        replace_bundle(&staged, &bundle).unwrap();
        assert_eq!(
            std::fs::read_to_string(bundle.join("version")).unwrap(),
            "new"
        );
        assert_eq!(
            std::fs::read_to_string(bundle.with_extension("app.old").join("version")).unwrap(),
            "old"
        );
        assert!(!staged.exists());
    }

    #[test]
    fn signature_payload_format() {
        // Contract: four \n-separated lines, no trailing newline.
        let payload = signature_payload(&manifest());
        assert_eq!(
            payload,
            b"0.1.1\nhttps://example.com/Removent-0.1.1-macos-arm64.zip\nab12\n1"
        );
    }

    #[test]
    fn signature_verify_positive_and_negative() {
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let key = signing.verifying_key();

        let mut m = manifest();
        let sig = signing.sign(&signature_payload(&m));
        m.signature = hex::encode(sig.to_bytes());
        assert!(verify_manifest_signature(&m, &key));

        // Tampered payload (different sha256 after signing).
        let mut tampered = m.clone();
        tampered.sha256 = "ff00".into();
        assert!(!verify_manifest_signature(&tampered, &key));

        // Wrong key.
        let other = SigningKey::from_bytes(&[9u8; 32]).verifying_key();
        assert!(!verify_manifest_signature(&m, &other));

        // Malformed signature encodings.
        let mut bad = m.clone();
        bad.signature = "not-hex".into();
        assert!(!verify_manifest_signature(&bad, &key));
        bad.signature = hex::encode([0u8; 32]); // wrong length
        assert!(!verify_manifest_signature(&bad, &key));
    }

    #[test]
    fn release_public_key_constant_parses() {
        assert!(release_verifying_key().is_some());
    }

    // ---- state-machine concurrency guards ----

    fn test_paths() -> DataPaths {
        DataPaths {
            root: std::env::temp_dir().join("removent-updater-test"),
        }
    }

    #[test]
    fn run_download_ignores_non_available_state() {
        // A second click while a download is already running must be a no-op
        // (and must not touch the network or the .part file).
        let shared = UpdateShared::new();
        shared.lock().unwrap().status = UpdateStatus::Downloading {
            version: "0.1.1".into(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        run_download(test_paths(), shared.clone(), tx);
        assert_eq!(
            shared.lock().unwrap().status,
            UpdateStatus::Downloading {
                version: "0.1.1".into()
            }
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn manual_check_keeps_ready_to_install() {
        // A staged update must survive even a manual re-check; overwriting it
        // would orphan the staged bundle and force a re-download.
        let shared = UpdateShared::new();
        shared.lock().unwrap().status = UpdateStatus::ReadyToInstall {
            version: "0.1.1".into(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        // An unreachable endpoint proves the guard returned before any fetch.
        run_check(
            "http://127.0.0.1:1/latest.json".into(),
            true,
            shared.clone(),
            tx,
        );
        assert_eq!(
            shared.lock().unwrap().status,
            UpdateStatus::ReadyToInstall {
                version: "0.1.1".into()
            }
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn run_install_ignores_non_ready_state() {
        let shared = UpdateShared::new();
        shared.lock().unwrap().status = UpdateStatus::Downloading {
            version: "0.1.1".into(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        run_install(shared.clone(), tx, Arc::new(Mutex::new(None)));
        assert_eq!(
            shared.lock().unwrap().status,
            UpdateStatus::Downloading {
                version: "0.1.1".into()
            }
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn run_install_claims_swap_atomically() {
        // ReadyToInstall + staged bundle claims the Swapping transition; the
        // swap itself fails in a dev environment (not running from a .app),
        // landing in Failed — but a concurrent second call sees Swapping and
        // returns instead of racing swap_bundle.
        let shared = UpdateShared::new();
        {
            let mut s = shared.lock().unwrap();
            s.status = UpdateStatus::ReadyToInstall {
                version: "0.1.1".into(),
            };
            s.staged_app = Some(PathBuf::from("/nonexistent/Removent.app"));
        }
        let (tx, _rx) = std::sync::mpsc::channel();
        run_install(shared.clone(), tx, Arc::new(Mutex::new(None)));
        match shared.lock().unwrap().status.clone() {
            UpdateStatus::Failed(_) => {}
            other => panic!("expected Failed after dev-env swap, got {other:?}"),
        }
    }
}

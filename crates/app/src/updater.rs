//! Client auto-update (release.md §3): GitHub Releases manifest check, signed
//! download, triple verification (sha256 + ed25519 + codesign), atomic .app swap
//! and relaunch.
//!
//! All network I/O shells out to `curl` (always present on macOS; the project
//! already shells out for `open`/`pgrep`) so no HTTP dependency is added.
//! Blocking work runs on the tokio blocking pool; the UI follows progress via
//! `UiEvent::UpdateStatus`.

use crate::engine::UiEvent;
use ed25519_dalek::{Verifier, VerifyingKey};
use removent_core::{DataPaths, Settings};
use rust_i18n::t;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

/// Stable manifest URL (the `latest.json` asset of the newest GitHub Release).
pub const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/backrunner/removent/releases/latest/download/latest.json";

/// Release-signing public key (Ed25519, hex). The private key only lives in the
/// CI secrets of the release pipeline (release.md §2).
const RELEASE_PUBLIC_KEY_HEX: &str =
    "2b51160721f9eee916944e4603e6540c628247e5284a79b099392f086a3def0c";

/// Expected Apple Developer Team ID for the codesign check. Releases are not yet
/// signed with a Developer ID certificate — fill in the Team ID once they are;
/// `None` skips the TeamID comparison.
const EXPECTED_TEAM_ID: Option<&str> = None;

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
/// the stable GitHub Releases URL.
pub fn manifest_endpoint(settings: &Settings) -> String {
    let custom = settings.update_endpoint.trim();
    if custom.is_empty() {
        DEFAULT_MANIFEST_URL.to_string()
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

/// Parse a three-segment numeric version ("1.2.3", optional leading "v").
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches('v');
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Semver-ish comparison: strictly newer three-segment version.
pub fn is_newer(remote: &str, local: &str) -> bool {
    match (parse_version(remote), parse_version(local)) {
        (Some(r), Some(l)) => r > l,
        _ => false,
    }
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
    key.verify(&signature_payload(m), &sig).is_ok()
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
        let status = &shared.lock().unwrap().status;
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
    }
    set_status(&shared, &events, UpdateStatus::Checking);
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
    let m: UpdateManifest = serde_json::from_str(&body)
        .map_err(|e| t!("update.err.bad_manifest", err = e.to_string()).to_string())?;
    // Signature before anything else: a tampered or unsigned manifest is
    // rejected before any download is even considered (release.md §3.2).
    let key = release_verifying_key().ok_or_else(|| t!("update.err.signature").to_string())?;
    if !verify_manifest_signature(&m, &key) {
        return Err(t!("update.err.signature").to_string());
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
            relaunch(&bundle);
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
    let backup = bundle.with_extension("app.old");
    let _ = std::fs::remove_dir_all(&backup);
    std::fs::rename(&bundle, &backup)
        .map_err(|e| t!("update.err.swap", err = e.to_string()).to_string())?;
    if let Err(e) = move_into_place(staged_app, &bundle) {
        // Roll back: the old bundle must never be left parked aside. If even
        // the rollback fails, name the backup path so the user can restore it
        // manually instead of silently sitting on `Removent.app.old`.
        if std::fs::rename(&backup, &bundle).is_err() {
            return Err(t!(
                "update.err.swap_rollback",
                err = e.to_string(),
                backup = backup.display().to_string()
            )
            .to_string());
        }
        return Err(t!("update.err.swap", err = e.to_string()).to_string());
    }
    Ok(bundle)
}

/// Move the staged bundle into the running bundle's place. rename(2) cannot
/// cross devices (EXDEV — e.g. the .app lives on an external volume while the
/// update cache is on the system volume), so fall back to copy + delete.
fn move_into_place(staged: &Path, bundle: &Path) -> std::io::Result<()> {
    match std::fs::rename(staged, bundle) {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            let copied = Command::new("ditto")
                .arg(staged)
                .arg(bundle)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()?;
            if !copied.success() {
                return Err(std::io::Error::other(format!("ditto exit {copied}")));
            }
            std::fs::remove_dir_all(staged)
        }
        Err(e) => Err(e),
    }
}

/// Restart the daemon (it ships inside the bundle; best-effort — it may not be
/// loaded at all), open the new app and exit this process. Never returns.
fn relaunch(bundle: &Path) -> ! {
    let uid = unsafe { libc::getuid() };
    let _ = Command::new("launchctl")
        .args(["kickstart", "-k", &format!("gui/{uid}/com.removent.daemon")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let _ = Command::new("open").arg("-n").arg(bundle).status();
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

fn curl_get(url: &str) -> Result<String, String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "30", url])
        .output()
        .map_err(|e| t!("update.err.fetch", err = e.to_string()).to_string())?;
    if !out.status.success() {
        return Err(t!(
            "update.err.fetch",
            err = format!("curl exit {}", out.status)
        )
        .to_string());
    }
    String::from_utf8(out.stdout)
        .map_err(|e| t!("update.err.bad_manifest", err = e.to_string()).to_string())
}

/// Download to a `.part` file; `resume` continues a partial download (`-C -`).
fn curl_download(url: &str, part: &Path, resume: bool) -> bool {
    let mut cmd = Command::new("curl");
    cmd.arg("-fSL")
        .arg("--max-time")
        .arg("600")
        .arg("-o")
        .arg(part);
    if resume {
        cmd.arg("-C").arg("-");
    }
    cmd.arg(url)
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
    std::fs::read_dir(stage).ok()?.find_map(|entry| {
        let path = entry.ok()?.path();
        (path.extension().is_some_and(|e| e == "app") && path.is_dir()).then_some(path)
    })
}

fn codesign_verify(app: &Path) -> Result<(), String> {
    let ok = Command::new("codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(app)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return Err(t!("update.err.codesign").to_string());
    }
    if let Some(team) = EXPECTED_TEAM_ID {
        // codesign prints the details (incl. TeamIdentifier=…) on stderr.
        let out = Command::new("codesign")
            .args(["-dv", "--verbose=4"])
            .arg(app)
            .output()
            .map_err(|e| t!("update.err.codesign", err = e.to_string()).to_string())?;
        let details = String::from_utf8_lossy(&out.stderr);
        let expected = format!("TeamIdentifier={team}");
        if !details.lines().any(|l| l.trim() == expected) {
            return Err(t!("update.err.codesign_team").to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

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
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("v0.10.2"), Some((0, 10, 2)));
        assert_eq!(parse_version(" 1.0.0 "), Some((1, 0, 0)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version("a.b.c"), None);

        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.1"));
        assert!(!is_newer("garbage", "0.1.0"));
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

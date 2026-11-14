//! Independent per-user recovery and unattended installation agent.
mod health;
pub(crate) mod io;
mod journal;
mod preflight;
mod registration;
#[cfg(test)]
mod runtime_tests;
mod supervisor;
#[cfg(test)]
mod tests;
mod transaction;

use crate::{UpdateManifest, UpdatePolicy};
use anyhow::{Context, Result, ensure};
use removent_core::{DataDirLock, DataPaths, Settings};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub use crate::AgentStatus;
#[derive(Serialize, Deserialize)]
struct Request {
    manifest: UpdateManifest,
    policy: UpdatePolicy,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn status_path(paths: &DataPaths) -> PathBuf {
    registration::directory(paths).join("status.json")
}
pub fn read_status(paths: &DataPaths) -> Result<AgentStatus> {
    io::load(&status_path(paths))
}
fn status(paths: &DataPaths, state: &str, version: Option<&str>, detail: &str) -> Result<()> {
    io::save(
        &status_path(paths),
        &AgentStatus {
            state: state.into(),
            version: version.map(str::to_owned),
            detail: detail.into(),
            updated_at: now(),
        },
    )
}
fn reject_version(paths: &DataPaths, version: &str) -> Result<()> {
    let file = registration::directory(paths).join("rejected.json");
    let mut versions: Vec<String> = if file.exists() {
        io::load(&file)?
    } else {
        Vec::new()
    };
    if !versions.iter().any(|v| v == version) {
        versions.push(version.into());
    }
    io::save(&file, &versions)
}
fn rejected(paths: &DataPaths, version: &str) -> Result<bool> {
    let file = registration::directory(paths).join("rejected.json");
    if !file.exists() {
        return Ok(false);
    }
    let versions: Vec<String> = io::load(&file)?;
    Ok(versions.iter().any(|v| v == version))
}
fn open_desktop(paths: &DataPaths, bundle: &Path) -> Result<()> {
    io::checked(
        Command::new("/usr/bin/open")
            .args(["-n", "--env"])
            .arg(format!("REMOVENT_DATA_DIR={}", paths.root.display()))
            .arg(bundle),
        Duration::from_secs(15),
    )?;
    Ok(())
}

pub fn request_install(
    paths: &DataPaths,
    bundle: &Path,
    manifest: &UpdateManifest,
    staged: &Path,
    policy: &UpdatePolicy,
) -> Result<()> {
    // Fail in the visible UI before it quits if approval/permissions are needed.
    preflight::installation(bundle, staged)?;
    registration::install(paths, bundle)?;
    io::save(
        &registration::directory(paths).join("request.json"),
        &Request {
            manifest: manifest.clone(),
            policy: policy.clone(),
        },
    )
}

fn check(paths: &DataPaths, bundle: &Path) -> Result<()> {
    let settings = Settings::load_strict(paths)?;
    let policy = UpdatePolicy::from_settings(&settings);
    let request_file = registration::directory(paths).join("request.json");
    let request: Option<Request> = if request_file.exists() {
        Some(io::load(&request_file)?)
    } else {
        None
    };
    if request.is_none() && (!settings.update_check_enabled || !settings.update_auto_install) {
        return status(
            paths,
            "disabled",
            None,
            "Automatic installation is disabled",
        );
    }
    // A shared lease probes the persistent recovery marker without blocking.
    if removent_core::update_gate::MaintenanceLease::acquire(paths).is_err() {
        return status(
            paths,
            "waiting_for_idle",
            None,
            "Close the main window and finish connections to install automatically",
        );
    }
    let current = crate::bundle_version(bundle).map_err(anyhow::Error::msg)?;
    let manual = request.is_some();
    if manual {
        io::remove(&request_file)?;
    }
    let manifest = if let Some(request) = request {
        ensure!(
            request.policy == policy,
            "Update channel changed; request the update again"
        );
        let key = crate::release_verifying_key().context("Missing update trust key")?;
        ensure!(
            crate::verify_manifest_signature(&request.manifest, &key),
            "Invalid update signature"
        );
        ensure!(
            crate::manifest::eligible_upgrade(policy.channel, &request.manifest.version, &current),
            "Requested version is not an upgrade"
        );
        Some(request.manifest)
    } else {
        let last: u64 =
            io::load(&registration::directory(paths).join("last-check.json")).unwrap_or(0);
        if now().saturating_sub(last) < 6 * 60 * 60 {
            return Ok(());
        }
        let result = crate::check::fetch_update(
            &policy,
            &current,
            &crate::release_verifying_key().context("Missing update trust key")?,
            crate::curl_get,
        );
        // Back off after failures as well; no repeated busy feed polling.
        io::save(
            &registration::directory(paths).join("last-check.json"),
            &now(),
        )?;
        result.map_err(anyhow::Error::msg)?
    };
    let Some(manifest) = manifest else {
        return status(paths, "up_to_date", None, "No newer release is available");
    };
    ensure!(
        !rejected(paths, &manifest.version)?,
        "This release failed startup checks; waiting for a newer repair release"
    );
    ensure!(
        manifest.min_compatible_proto <= removent_proto::PROTO_VERSION,
        "This release requires a newer update protocol"
    );
    status(
        paths,
        "downloading",
        Some(&manifest.version),
        "Downloading and verifying update",
    )?;
    let cache = DataPaths {
        root: registration::directory(paths).join("download"),
    };
    cache.ensure_layout()?;
    let shared = crate::UpdateShared::new(&settings);
    let (events, _receiver) = std::sync::mpsc::channel();
    let staged = crate::download::download_and_stage(&cache, &manifest, &shared, &events)
        .map_err(anyhow::Error::msg)?;
    // Re-read persistent user choice after the download and again inside the
    // admission gate; channel changes and opt-out cancel pending installation.
    let latest = Settings::load_strict(paths)?;
    ensure!(
        UpdatePolicy::from_settings(&latest) == policy
            && (manual || (latest.update_check_enabled && latest.update_auto_install)),
        "Update preferences changed; installation cancelled"
    );
    status(
        paths,
        "verifying",
        Some(&manifest.version),
        "Checking startup before installation",
    )?;
    let result = transaction::install(paths, bundle, &staged, &manifest.version, manual, &policy);
    if manual {
        io::remove(&request_file)?;
    }
    result
}

fn serve(paths: &DataPaths, bundle: &Path) -> Result<()> {
    let dir = registration::directory(paths);
    io::private_dir(&dir)?;
    paths.ensure_layout()?;
    let _lock = DataDirLock::acquire_at(&dir.join("agent.lock"))?;
    let registered: PathBuf = io::load(&dir.join("installation.json"))?;
    ensure!(
        registered == bundle,
        "Updater registration does not match this installation"
    );
    let arguments: Vec<_> = std::env::args().collect();
    if let Some(index) = arguments.iter().position(|a| a == "--supervisor-pid") {
        let parent: i32 = arguments
            .get(index + 1)
            .context("Missing supervisor PID")?
            .parse()?;
        std::thread::spawn(move || {
            loop {
                if unsafe { libc::getppid() } != parent {
                    std::process::exit(1);
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }
    let mut sequence = 0u64;
    supervisor::heartbeat(paths, sequence)?;
    loop {
        let result = transaction::recover(paths, bundle).and_then(|()| check(paths, bundle));
        if let Err(error) = result {
            tracing::warn!(error=%format!("{error:#}"), "update deferred or recovered");
            let _ = status(paths, "deferred", None, &format!("{error:#}"));
        }
        sequence = sequence.wrapping_add(1);
        supervisor::heartbeat(paths, sequence)?;
        if supervisor::replacement_pending(paths)? {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(30));
    }
}

pub fn entry() -> Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|v| v == "--self-test") {
        ensure!(
            crate::release_verifying_key().is_some(),
            "Update trust key is invalid"
        );
        println!("removent-update {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    ensure!(
        unsafe { libc::geteuid() } != 0,
        "Run the recovery agent as the logged-in user, never as root"
    );
    let paths = DataPaths::resolve();
    removent_core::logging::init_logging(&paths);
    removent_core::logging::install_panic_hook(&paths);
    match args.get(1).and_then(|v| v.to_str()) {
        Some("--install") => registration::install(
            &paths,
            Path::new(args.get(2).context("Missing bundle path")?),
        ),
        Some("--supervise") => supervisor::run(
            &paths,
            Path::new(args.get(2).context("Missing bundle path")?),
        ),
        Some("--serve") => serve(
            &paths,
            Path::new(args.get(2).context("Missing bundle path")?),
        ),
        Some("--status") => {
            println!("{}", serde_json::to_string_pretty(&read_status(&paths)?)?);
            Ok(())
        }
        _ => anyhow::bail!(
            "Usage: removent-update --install APP | --serve APP | --status | --self-test"
        ),
    }
}

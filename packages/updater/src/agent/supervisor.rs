//! A pinned supervisor watches replaceable signed update workers. App upgrades
//! never replace this executable or the last known working worker in place.
use super::{io, preflight, registration};
use anyhow::{Context, Result, ensure};
use removent_core::{DataDirLock, DataPaths};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Default, Serialize, Deserialize)]
struct Runtime {
    current: Option<String>,
    previous: Option<String>,
    #[serde(default)]
    rejected: Vec<String>,
}
#[derive(Serialize, Deserialize)]
struct Heartbeat {
    pid: u32,
    sequence: u64,
}

fn worker_path(dir: &Path, digest: &str) -> Result<PathBuf> {
    ensure!(
        digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid update worker identity"
    );
    Ok(dir.join("workers").join(digest))
}

/// Called only on initial bootstrap or AFTER an application's health commit.
pub fn stage(paths: &DataPaths, bundle: &Path) -> Result<()> {
    let dir = registration::directory(paths);
    let _lock = DataDirLock::acquire_at(&dir.join("worker-store.lock"))?;
    io::private_dir(&dir.join("workers"))?;
    let source = bundle.join("Contents/MacOS/removent-update");
    preflight::verify_component(&source, "com.alkinum.removent.update")?;
    let digest = crate::sha256_file(&source).map_err(anyhow::Error::msg)?;
    let state = dir.join("runtime.json");
    let runtime: Runtime = if state.exists() {
        io::load(&state)?
    } else {
        Runtime::default()
    };
    if runtime.current.as_ref() == Some(&digest) || runtime.rejected.contains(&digest) {
        return Ok(());
    }
    let target = worker_path(&dir, &digest)?;
    if !target.exists() {
        let incoming = tempfile::NamedTempFile::new_in(dir.join("workers"))?;
        std::fs::copy(source, incoming.path())?;
        incoming.as_file().sync_all()?;
        incoming.persist(&target)?;
        io::sync_dir(&dir.join("workers"))?;
    }
    ensure!(
        crate::sha256_file(&target).map_err(anyhow::Error::msg)? == digest,
        "Cached update worker changed"
    );
    preflight::verify_component(&target, "com.alkinum.removent.update")?;
    io::checked(
        Command::new(&target).arg("--self-test"),
        Duration::from_secs(10),
    )?;
    io::save(&dir.join("pending-worker.json"), &digest)
}

pub fn heartbeat(paths: &DataPaths, sequence: u64) -> Result<()> {
    io::save(
        &registration::directory(paths).join("worker-heartbeat.json"),
        &Heartbeat {
            pid: std::process::id(),
            sequence,
        },
    )
}
pub fn replacement_pending(paths: &DataPaths) -> Result<bool> {
    let pending = registration::directory(paths).join("pending-worker.json");
    if !pending.exists() {
        return Ok(false);
    }
    let digest: String = io::load(&pending)?;
    Ok(crate::sha256_file(&std::env::current_exe()?).map_err(anyhow::Error::msg)? != digest)
}

pub fn run(paths: &DataPaths, bundle: &Path) -> Result<()> {
    let dir = registration::directory(paths);
    let _lock = DataDirLock::acquire_at(&dir.join("supervisor.lock"))?;
    let pinned = dir.join("removent-update");
    let state_file = dir.join("runtime.json");
    let pending_file = dir.join("pending-worker.json");
    loop {
        let mut runtime: Runtime = if state_file.exists() {
            io::load(&state_file)?
        } else {
            Runtime::default()
        };
        let pending: Option<String> = if pending_file.exists() {
            Some(io::load(&pending_file)?)
        } else {
            None
        };
        if pending
            .as_ref()
            .is_some_and(|p| runtime.rejected.contains(p))
        {
            io::remove(&pending_file)?;
        }
        let candidate = pending.filter(|p| !runtime.rejected.contains(p));
        let selected = candidate.clone().or_else(|| runtime.current.clone());
        let executable = selected
            .as_deref()
            .map(|s| worker_path(&dir, s))
            .transpose()?
            .unwrap_or_else(|| pinned.clone());
        let trial = selected != runtime.current;
        let result = monitor(paths, bundle, &executable, |()| {
            if trial {
                runtime.previous = runtime.current.take();
                runtime.current = selected.clone();
                io::save(&state_file, &runtime)?;
                // A newer worker could have been queued while this one started.
                if pending_file.exists()
                    && io::load::<String>(&pending_file).ok().as_ref() == selected.as_ref()
                {
                    io::remove(&pending_file)?;
                }
                retire_workers(&dir, &runtime);
            }
            Ok(())
        });
        // Workers exit normally only at an idle boundary to activate a pending
        // worker. A spontaneous exit, including exit(0), triggers fallback.
        let handing_off = result.as_ref().is_ok_and(|clean| *clean)
            && pending_file.exists()
            && io::load::<String>(&pending_file).ok().as_ref() != selected.as_ref();
        if !handing_off {
            if let Some(failed) = selected {
                if !runtime.rejected.contains(&failed) {
                    runtime.rejected.push(failed.clone());
                }
                if runtime.current.as_ref() == Some(&failed) {
                    runtime.current = runtime.previous.take();
                }
                if pending_file.exists()
                    && io::load::<String>(&pending_file).ok().as_ref() == Some(&failed)
                {
                    io::remove(&pending_file)?;
                }
                io::save(&state_file, &runtime)?;
            }
            let _ = super::status(
                paths,
                "agent_recovered",
                None,
                "The update worker stopped; the previous recovery worker will resume.",
            );
            tracing::warn!(?result, "update worker fallback");
            std::thread::sleep(Duration::from_secs(5));
        }
    }
}

fn retire_workers(dir: &Path, runtime: &Runtime) {
    // Staging takes this same lock, so a new immutable file cannot disappear
    // between publication and the atomic pending-worker pointer write.
    let Ok(_lock) = DataDirLock::acquire_at(&dir.join("worker-store.lock")) else {
        return;
    };
    let pending: Option<String> = io::load(&dir.join("pending-worker.json")).ok();
    let Ok(entries) = std::fs::read_dir(dir.join("workers")) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.len() == 64
            && name.bytes().all(|b| b.is_ascii_hexdigit())
            && [
                runtime.current.as_deref(),
                runtime.previous.as_deref(),
                pending.as_deref(),
            ]
            .iter()
            .all(|keep| *keep != Some(name))
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn monitor(
    paths: &DataPaths,
    bundle: &Path,
    executable: &Path,
    mut healthy: impl FnMut(()) -> Result<()>,
) -> Result<bool> {
    preflight::verify_component(executable, "com.alkinum.removent.update")?;
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Child(
        Command::new(executable)
            .arg("--serve")
            .arg(bundle)
            .arg("--supervisor-pid")
            .arg(std::process::id().to_string())
            .env("REMOVENT_DATA_DIR", &paths.root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let started = Instant::now();
    let mut last_progress = started;
    let mut last_sequence = None;
    let mut maintenance_since = None;
    let mut promoted = false;
    loop {
        if let Some(status) = child.0.try_wait()? {
            return Ok(status.success());
        }
        let receipt: Option<Heartbeat> =
            io::load(&registration::directory(paths).join("worker-heartbeat.json")).ok();
        if let Some(receipt) = receipt.filter(|h| h.pid == child.0.id())
            && last_sequence != Some(receipt.sequence)
        {
            last_sequence = Some(receipt.sequence);
            last_progress = Instant::now();
        }
        if !promoted
            && started.elapsed() >= Duration::from_secs(65)
            && last_sequence.is_some_and(|s| s >= 2)
        {
            healthy(()).context("Could not commit healthy update worker")?;
            promoted = true;
        }
        if paths.run_dir().join("update-maintenance").exists() {
            let since = maintenance_since.get_or_insert_with(Instant::now);
            ensure!(
                since.elapsed() < Duration::from_secs(300),
                "Update worker stalled during installation; recovering"
            );
        } else {
            maintenance_since = None;
        }
        ensure!(
            last_progress.elapsed() < Duration::from_secs(1800),
            "Update worker stopped responding; recovering"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
}

use super::io;
use anyhow::{Context, Result, ensure};
use removent_core::{
    DataPaths, Settings,
    ipc::{IpcResponse, StatusReport},
};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Health {
    pub version: String,
    pub pid: u32,
    pub status: StatusReport,
}

pub fn read(paths: &DataPaths) -> Result<Health> {
    let mut stream = UnixStream::connect(paths.daemon_socket())?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(b"{\"type\":\"update_health\"}\n")?;
    let mut reader = BufReader::new(stream).take(1024 * 1024);
    for _ in 0..64 {
        let mut line = String::new();
        ensure!(
            reader.read_line(&mut line)? != 0,
            "Host disconnected during health check"
        );
        if let Ok(IpcResponse::UpdateHealth {
            version,
            pid,
            status,
        }) = serde_json::from_str(&line)
        {
            return Ok(Health {
                version,
                pid,
                status: *status,
            });
        }
    }
    anyhow::bail!("Host did not answer the update health check")
}

pub fn validate(current: &Health, version: &str, previous: Option<&Health>) -> Result<()> {
    ensure!(
        current.version == version && current.pid > 0,
        "The expected host version did not start"
    );
    ensure!(
        !current.status.running
            || (current.status.host_ready && current.status.host_error.is_none()),
        "Host listener did not become ready"
    );
    ensure!(
        !current.status.running
            || (current.status.screen_recording_granted && current.status.accessibility_granted),
        "Host permissions are unavailable; open System Settings before updating"
    );
    if let Some(before) = previous {
        ensure!(
            before.status.fp_short == current.status.fp_short,
            "Host identity changed during update"
        );
        ensure!(
            !before.status.screen_recording_granted || current.status.screen_recording_granted,
            "Screen Recording permission was lost; open System Settings before updating"
        );
        ensure!(
            !before.status.accessibility_granted || current.status.accessibility_granted,
            "Accessibility permission was lost; open System Settings before updating"
        );
    }
    Ok(())
}

pub fn probation(
    paths: &DataPaths,
    version: &str,
    previous: Option<&Health>,
    duration: Duration,
) -> Result<()> {
    let ready_deadline = Instant::now() + Duration::from_secs(20);
    let first = loop {
        match read(paths).and_then(|h| {
            validate(&h, version, previous)?;
            Ok(h)
        }) {
            Ok(h) => break h,
            Err(e) if Instant::now() >= ready_deadline => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }
    };
    let until = Instant::now() + duration;
    while Instant::now() < until {
        std::thread::sleep(Duration::from_secs(1));
        let h = read(paths)?;
        validate(&h, version, previous)?;
        ensure!(
            h.pid == first.pid,
            "Host restarted during the health observation period"
        );
    }
    Ok(())
}

/// Real UI initialization, isolated from credentials, preferences and services.
/// Success requires the first rendered frame, ten seconds alive and clean exit.
pub fn desktop_smoke(bundle: &Path, version: &str) -> Result<()> {
    let temp = tempfile::tempdir()?;
    let paths = DataPaths {
        root: temp.path().into(),
    };
    paths.ensure_layout()?;
    Settings {
        host_enabled: false,
        update_check_enabled: false,
        update_auto_install: false,
        ..Default::default()
    }
    .save(&paths)?;
    io::checked(
        Command::new(bundle.join("Contents/MacOS/removent"))
            .arg("--update-smoke")
            .env("REMOVENT_DATA_DIR", &paths.root),
        Duration::from_secs(35),
    )?;
    ensure!(
        std::fs::read_to_string(paths.run_dir().join("update-smoke-ok"))
            .context("Desktop did not render a frame")?
            .trim()
            == version,
        "Desktop startup version check failed"
    );
    Ok(())
}

pub fn daemon_smoke(bundle: &Path, version: &str) -> Result<()> {
    let temp = tempfile::tempdir()?;
    let paths = DataPaths {
        root: temp.path().into(),
    };
    paths.ensure_layout()?;
    Settings {
        host_enabled: false,
        update_check_enabled: false,
        ..Default::default()
    }
    .save(&paths)?;
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Child(
        Command::new(bundle.join("Contents/MacOS/removentd"))
            .env("REMOVENT_DATA_DIR", &paths.root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?,
    );
    probation(&paths, version, None, Duration::from_secs(10))?;
    ensure!(child.0.try_wait()?.is_none(), "Host probe exited early");
    unsafe {
        libc::kill(child.0.id() as i32, libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.0.try_wait()? {
            ensure!(status.success(), "Host probe did not exit cleanly");
            break;
        }
        ensure!(Instant::now() < deadline, "Host probe shutdown timed out");
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn healthy() -> Health {
        Health {
            version: "2.0.0".into(),
            pid: 123,
            status: StatusReport {
                running: true,
                host_ready: true,
                fp_short: "identity".into(),
                screen_recording_granted: true,
                accessibility_granted: true,
                ..Default::default()
            },
        }
    }
    #[test]
    fn permission_loss_identity_change_and_stale_host_fail_health() {
        let old = healthy();
        for mutate in [
            |h: &mut Health| h.status.screen_recording_granted = false,
            |h: &mut Health| h.status.accessibility_granted = false,
            |h: &mut Health| h.status.fp_short = "new".into(),
            |h: &mut Health| h.status.host_ready = false,
            |h: &mut Health| h.version = "1.0.0".into(),
        ] {
            let mut h = healthy();
            mutate(&mut h);
            assert!(validate(&h, "2.0.0", Some(&old)).is_err());
        }
        assert!(validate(&healthy(), "2.0.0", Some(&old)).is_ok());
    }

    #[test]
    fn launchd_restart_during_probation_is_a_failure_even_if_ipc_recovers() {
        use std::os::unix::net::UnixListener;
        let temp = tempfile::tempdir().unwrap();
        let paths = DataPaths {
            root: temp.path().into(),
        };
        paths.ensure_layout().unwrap();
        let listener = UnixListener::bind(paths.daemon_socket()).unwrap();
        let server = std::thread::spawn(move || {
            for pid in [123, 456] {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = String::new();
                BufReader::new(socket.try_clone().unwrap())
                    .read_line(&mut request)
                    .unwrap();
                let message = IpcResponse::UpdateHealth {
                    version: "2.0.0".into(),
                    pid,
                    status: Box::new(healthy().status),
                };
                writeln!(socket, "{}", serde_json::to_string(&message).unwrap()).unwrap();
            }
        });
        let error = probation(&paths, "2.0.0", None, Duration::from_secs(2)).unwrap_err();
        assert!(error.to_string().contains("restarted"));
        server.join().unwrap();
    }
}

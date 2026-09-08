//! User-session launchd management, shared by the app and packaged CLI/tray.
//! A temporary job survives the UI exiting; only login-on persists it for login.

use crate::{DataDirLock, DataPaths};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

pub const LABEL: &str = "com.removent.daemon";

pub struct Service {
    pub paths: DataPaths,
    executable: PathBuf,
    login_file: PathBuf,
    label: String,
}

#[derive(Serialize)]
pub struct ServiceStatus {
    pub launch_at_login: bool,
    pub managed: bool,
    pub reachable: bool,
}

impl Service {
    pub fn new(paths: DataPaths, executable: PathBuf) -> Result<Self> {
        // Canonicalize only after creating the root: /tmp is a symlink on
        // macOS, and a not-yet-existing custom root must get the same job label
        // on its first start and all subsequent management commands.
        paths.ensure_layout()?;
        let home = std::env::var_os("HOME").context("HOME is unavailable")?;
        let home = PathBuf::from(home);
        let default_data = home.join("Library/Application Support/removent/userdata");
        let normalize = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        let label = if normalize(&paths.root) == normalize(&default_data) {
            LABEL.into()
        } else {
            // Development/custom data directories must never control the
            // installed application's job or replace its login registration.
            use sha2::{Digest, Sha256};
            let digest = hex::encode(Sha256::digest(
                normalize(&paths.root).as_os_str().as_encoded_bytes(),
            ));
            format!("{LABEL}.{}", &digest[..12])
        };
        Ok(Self {
            paths,
            executable,
            login_file: home.join(format!("Library/LaunchAgents/{label}.plist")),
            label,
        })
    }

    fn domain(&self) -> String {
        format!("gui/{}", unsafe { libc::getuid() })
    }

    fn target(&self) -> String {
        format!("{}/{}", self.domain(), self.label)
    }

    fn launchctl(&self, args: &[&str]) -> Result<Output> {
        Command::new("/bin/launchctl")
            .args(args)
            .output()
            .context("Could not run launchctl")
    }

    fn checked(&self, args: &[&str]) -> Result<()> {
        let out = self.launchctl(args)?;
        ensure!(
            out.status.success(),
            "launchctl {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(())
    }

    pub fn status(&self) -> Result<ServiceStatus> {
        Ok(ServiceStatus {
            // Persistence and runtime state are separate: deleting the login
            // registration must not terminate an active remote session.
            launch_at_login: self.login_file.is_file(),
            managed: self.launchctl(&["print", &self.target()])?.status.success(),
            reachable: std::os::unix::net::UnixStream::connect(self.paths.daemon_socket()).is_ok(),
        })
    }

    fn lock(&self) -> Result<DataDirLock> {
        self.paths.ensure_layout()?;
        DataDirLock::acquire_at(&self.paths.run_dir().join("service.lock"))
            .context("Another service operation is in progress; try again shortly")
    }

    fn plist(&self) -> Result<String> {
        let exe = self
            .executable
            .canonicalize()
            .context("removentd was not found")?;
        let data = self.paths.root.canonicalize()?;
        Ok(render_plist(&self.label, &exe, &data))
    }

    /// Start independently of both UI processes. Never launch a competing
    /// instance against a live pre-launchd daemon from an older application.
    pub fn start(&self) -> Result<()> {
        let _lock = self.lock()?;
        self.start_locked()
    }

    fn start_locked(&self) -> Result<()> {
        let status = self.status()?;
        if status.reachable {
            return Ok(());
        }
        if status.managed {
            self.checked(&["kickstart", &self.target()])?;
        } else {
            let plist = self.plist()?;
            let transient = self.paths.run_dir().join(format!("{}.plist", self.label));
            crate::settings::atomic_write(&transient, plist.as_bytes())?;
            self.checked(&["enable", &self.target()])?;
            self.checked(&[
                "bootstrap",
                &self.domain(),
                transient.to_str().context("Invalid plist path")?,
            ])?;
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.status()?.reachable {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        bail!(
            "removentd did not become ready; check {}/logs and System Settings > General > Login Items",
            self.paths.root.display()
        )
    }

    /// Register future logins without restarting a running service. Existing
    /// unmanaged instances are adopted on the next login or explicit restart.
    pub fn set_launch_at_login(&self, on: bool) -> Result<()> {
        let _lock = self.lock()?;
        if on {
            let plist = self.plist()?;
            std::fs::create_dir_all(self.login_file.parent().unwrap())?;
            let previous = std::fs::read(&self.login_file).ok();
            crate::settings::atomic_write(&self.login_file, plist.as_bytes())?;
            if let Err(error) = self.start_locked() {
                if let Some(previous) = previous {
                    crate::settings::atomic_write(&self.login_file, &previous)?;
                } else {
                    std::fs::remove_file(&self.login_file)?;
                }
                return Err(error);
            }
        } else {
            match std::fs::remove_file(&self.login_file) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// Explicit process stop/restart. bootout removes KeepAlive before waiting
    /// for graceful shutdown, so launchd cannot race the replacement process.
    pub fn stop(&self) -> Result<()> {
        let _lock = self.lock()?;
        self.stop_locked()
    }

    pub fn restart(&self) -> Result<()> {
        let _lock = self.lock()?;
        self.stop_locked()?;
        self.start_locked()
    }

    fn stop_locked(&self) -> Result<()> {
        let status = self.status()?;
        if status.managed {
            self.checked(&["bootout", &self.target()])?;
        } else if status.reachable {
            // The old app may have spawned removentd directly. Target its
            // authenticated per-user socket, never a name-wide pkill.
            use std::io::{BufRead, BufReader, Read, Write};
            let mut socket = std::os::unix::net::UnixStream::connect(self.paths.daemon_socket())?;
            socket.set_read_timeout(Some(Duration::from_secs(3)))?;
            socket.set_write_timeout(Some(Duration::from_secs(3)))?;
            socket.write_all(b"{\"type\":\"shutdown\"}\n")?;
            let mut reply = String::new();
            BufReader::new(socket).take(1 << 20).read_line(&mut reply)?;
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(lock) = DataDirLock::acquire_at(&self.paths.daemon_lock_file()) {
                drop(lock);
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "removentd is still shutting down"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn render_plist(label: &str, executable: &Path, data: &Path) -> String {
    fn xml(value: &str) -> String {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }
    include_str!("../../../scripts/com.removent.daemon.plist")
        .replace("com.removent.daemon", &xml(label))
        .replace("@EXE@", &xml(&executable.to_string_lossy()))
        .replace("@DATA@", &xml(&data.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_custom_directory_keeps_its_job_label() {
        let tmp = tempfile::tempdir_in("/tmp").unwrap();
        let paths = DataPaths {
            root: tmp.path().join("not-created-yet"),
        };
        let first = Service::new(paths.clone(), "/bin/true".into()).unwrap();
        let next = Service::new(paths, "/bin/true".into()).unwrap();
        assert_eq!(first.label, next.label);
        assert_eq!(first.login_file, next.login_file);
        assert_ne!(first.label, LABEL);
    }

    #[test]
    fn plist_handles_spaces_and_xml_in_paths() {
        let text = render_plist(
            LABEL,
            Path::new("/Applications/Test & <New>.app/removentd"),
            Path::new("/Users/a b/data"),
        );
        assert!(text.contains("Test &amp; &lt;New&gt;.app/removentd"));
        assert!(text.contains("/Users/a b/data/logs/removentd.err.log"));
        assert!(!text.contains("@EXE@"));
        assert!(!text.contains("@DATA@"));
    }

    /// Uses a private label, fake server, data directory and login directory.
    /// No personal Removent process or login registration is touched.
    #[test]
    #[ignore = "requires a macOS GUI launchd session"]
    fn launchd_lifecycle() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::Builder::new()
            .prefix("removent-svc-")
            .tempdir_in("/tmp")
            .unwrap();
        let paths = DataPaths {
            root: tmp.path().join("data"),
        };
        let exe = tmp.path().join("fake-server");
        std::fs::write(
            &exe,
            r#"#!/usr/bin/python3
import fcntl, os, pathlib, signal, socket, sys
root = pathlib.Path(os.environ['REMOVENT_DATA_DIR'])
lock = (root / '.daemon.lock').open('w')
fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
path = root / 'run/removentd.sock'
path.unlink(missing_ok=True)
server = socket.socket(socket.AF_UNIX)
server.bind(str(path))
server.listen()
(root / 'pid').write_text(str(os.getpid()))
def stop(*args):
    path.unlink(missing_ok=True)
    sys.exit(0)
signal.signal(signal.SIGTERM, stop)
while True:
    client, _ = server.accept()
    client.close()
"#,
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut service = Service::new(paths.clone(), exe).unwrap();
        service.login_file = tmp.path().join("LaunchAgents/test.plist");
        struct Cleanup<'a>(&'a Service);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                let _ = self.0.launchctl(&["bootout", &self.0.target()]);
            }
        }
        let _cleanup = Cleanup(&service);
        service.start().unwrap();
        assert!(service.status().unwrap().managed);
        assert!(service.status().unwrap().reachable);
        assert!(!service.status().unwrap().launch_at_login);
        let pid = || std::fs::read_to_string(paths.root.join("pid")).unwrap();
        let original_pid = pid();
        service.start().unwrap();
        service.set_launch_at_login(true).unwrap();
        assert!(service.status().unwrap().launch_at_login);
        service.set_launch_at_login(false).unwrap();
        assert!(!service.status().unwrap().launch_at_login);
        assert_eq!(
            pid(),
            original_pid,
            "login switches must not interrupt a session"
        );
        let parsed: i32 = original_pid.parse().unwrap();
        assert_eq!(unsafe { libc::kill(parsed, libc::SIGKILL) }, 0);
        let deadline = Instant::now() + Duration::from_secs(25);
        while pid() == original_pid || !service.status().unwrap().reachable {
            assert!(
                Instant::now() < deadline,
                "launchd did not recover the crashed server"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        service.restart().unwrap();
        assert!(service.status().unwrap().reachable);
        service.stop().unwrap();
        assert!(!service.status().unwrap().reachable);
        assert!(!service.status().unwrap().managed);
    }
}

//! Per-user launchd service, independent of the Removent desktop daemon.
use super::*;
use anyhow::bail;
use serde::{Deserialize, Serialize};
use std::{
    os::fd::AsRawFd,
    process::{Command, Output},
    time::{Duration, Instant},
};

const LABEL: &str = "com.alkinum.removent.relay";
const MARKER: &str = "<!-- Managed by removent-relay setup -->";

fn home() -> Result<PathBuf> {
    ensure!(
        unsafe { libc::geteuid() } != 0,
        "Run macOS relay commands as your login user, without sudo"
    );
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME is unavailable")?);
    ensure!(home.is_absolute(), "HOME must be absolute");
    Ok(home)
}

pub(super) fn config_dir(custom: Option<&Path>) -> Result<PathBuf> {
    Ok(match custom {
        Some(path) => std::path::absolute(path)?,
        None => home()?.join("Library/Application Support/removent-relay"),
    })
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Installation {
    version: u32,
    executable: PathBuf,
}

pub(super) struct Service {
    dir: PathBuf,
    label: String,
    login_file: PathBuf,
}

struct Job {
    pid: Option<u32>,
}

impl Service {
    pub(super) fn new(custom: Option<&Path>) -> Result<Self> {
        let home = home()?;
        let dir = config_dir(custom)?;
        // Resolve symlinked ancestors before hashing even on first setup.
        let dir = normalize(&dir)?;
        let default = normalize(&config_dir(None)?)?;
        let label = if dir == default {
            LABEL.to_string()
        } else {
            let hash = hex::encode(Sha256::digest(dir.as_os_str().as_encoded_bytes()));
            format!("{LABEL}.{}", &hash[..12])
        };
        let login_file = home.join(format!("Library/LaunchAgents/{label}.plist"));
        Ok(Self {
            dir,
            label,
            login_file,
        })
    }

    fn domain(&self) -> String {
        format!("gui/{}", unsafe { libc::geteuid() })
    }
    fn target(&self) -> String {
        format!("{}/{}", self.domain(), self.label)
    }
    fn run_dir(&self) -> PathBuf {
        self.dir.join("run")
    }
    fn ready_file(&self) -> PathBuf {
        self.run_dir().join("ready.pid")
    }
    fn transient(&self) -> PathBuf {
        self.run_dir().join("service.plist")
    }
    fn config(&self) -> PathBuf {
        self.dir.join("server.toml")
    }

    fn launchctl(&self, args: &[&str]) -> Result<Output> {
        Command::new("/bin/launchctl")
            .args(args)
            .output()
            .context("Cannot run launchctl")
    }
    fn checked(&self, args: &[&str]) -> Result<()> {
        let output = self.launchctl(args)?;
        ensure!(
            output.status.success(),
            "launchctl {} failed: {}",
            args[0],
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Ok(())
    }
    fn require_session(&self) -> Result<()> {
        ensure!(
            self.launchctl(&["print", &self.domain()])?.status.success(),
            "Log in to a macOS desktop session before managing the LaunchAgent, or use serve CONFIG for foreground operation"
        );
        Ok(())
    }
    fn installation(&self) -> Result<Installation> {
        let record: Installation =
            serde_json::from_str(&read_private(&self.dir.join("service.json"), 16384)?)
                .context("Service not installed; run removent-relay setup")?;
        ensure!(
            record.version == 1 && record.executable.is_absolute(),
            "Invalid service installation record"
        );
        Ok(record)
    }
    fn lock(&self) -> Result<File> {
        private_dir(&self.dir)?;
        private_dir(&self.run_dir())?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.run_dir().join("management.lock"))?;
        let meta = file.metadata()?;
        ensure!(
            meta.is_file() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0,
            "Unsafe service management lock"
        );
        ensure!(
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
            "Another relay management command is in progress"
        );
        Ok(file)
    }
    fn installed_login_file(&self) -> Result<bool> {
        if !self.login_file.try_exists()? && !self.login_file.is_symlink() {
            return Ok(false);
        }
        let body = read_private(&self.login_file, 32768)?;
        ensure!(
            body.contains(MARKER) && body.contains(&format!("<string>{}</string>", self.label)),
            "Existing LaunchAgent is not managed by this installation; refusing to modify it"
        );
        Ok(true)
    }
    fn job(&self) -> Result<Option<Job>> {
        let output = self.launchctl(&["print", &self.target()])?;
        if !output.status.success() {
            return Ok(None);
        }
        let text = String::from_utf8(output.stdout)?;
        let field = |name: &str| {
            text.lines()
                .map(str::trim)
                .find_map(|line| line.strip_prefix(name))
        };
        let record = self.installation()?;
        ensure!(
            field("program = ") == record.executable.to_str(),
            "A different program owns this launchd label"
        );
        let path = field("path = ").context("Cannot determine the loaded LaunchAgent path")?;
        ensure!(
            Path::new(path) == self.transient() || Path::new(path) == self.login_file,
            "A different LaunchAgent owns this label"
        );
        let pid = field("pid = ")
            .map(|value| value.parse::<u32>())
            .transpose()?;
        Ok(Some(Job { pid }))
    }
    fn ready(&self, job: &Job) -> bool {
        job.pid.is_some_and(|pid| {
            read_private(&self.ready_file(), 32).ok().as_deref() == Some(&pid.to_string())
        })
    }
    fn definition(&self) -> Result<String> {
        let record = self.installation()?;
        let meta = fs::metadata(&record.executable).context("Installed relay binary is missing")?;
        ensure!(
            meta.is_file()
                && meta.mode() & 0o022 == 0
                && (meta.uid() == 0 || meta.uid() == unsafe { libc::geteuid() }),
            "Unsafe relay executable permissions"
        );
        Ok(render_plist(&self.label, &record.executable, &self.dir))
    }
    fn prepare_logs(&self) -> Result<()> {
        let directory = self.dir.join("logs");
        private_dir(&directory)?;
        for name in ["relay.log", "relay.err.log"] {
            let path = directory.join(name);
            if path.try_exists()? || path.is_symlink() {
                // Validate before launchd opens its log paths. Rotate only while
                // stopped, so an active daemon never writes to a deleted file.
                let file = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                    .open(&path)?;
                let meta = file.metadata()?;
                ensure!(
                    meta.is_file()
                        && meta.uid() == unsafe { libc::geteuid() }
                        && meta.mode() & 0o077 == 0,
                    "Unsafe relay log file"
                );
                if meta.len() >= 10 * 1024 * 1024 {
                    fs::rename(&path, directory.join(format!("{name}.1")))?;
                }
            }
            if !path.exists() {
                write_new(&path, "")?;
            }
        }
        Ok(())
    }
    fn set_login(&self, enabled: bool) -> Result<()> {
        let present = self.installed_login_file()?;
        if enabled {
            let parent = self.login_file.parent().unwrap();
            fs::create_dir_all(parent)?;
            let meta = fs::symlink_metadata(parent)?;
            ensure!(
                meta.is_dir()
                    && meta.uid() == unsafe { libc::geteuid() }
                    && meta.mode() & 0o022 == 0,
                "LaunchAgents directory must be owned by the current user and not group/world writable"
            );
            atomic_write(&self.login_file, self.definition()?.as_bytes())?;
            self.checked(&["enable", &self.target()])?;
        } else if present {
            fs::remove_file(&self.login_file)?;
        }
        Ok(())
    }
    fn start(&self) -> Result<()> {
        read_config(&self.config())?;
        let loaded = self.job()?;
        if loaded.as_ref().is_some_and(|job| self.ready(job)) {
            return Ok(());
        }
        if loaded.is_some() {
            self.checked(&["kickstart", &self.target()])?;
        } else {
            let ready = self.ready_file();
            if ready.exists() || ready.is_symlink() {
                fs::remove_file(ready)?;
            }
            self.prepare_logs()?;
            atomic_write(&self.transient(), self.definition()?.as_bytes())?;
            self.checked(&["enable", &self.target()])?;
            self.checked(&["bootstrap", &self.domain(), path_str(&self.transient())?])?;
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if let Some(job) = self.job()?
                && self.ready(&job)
            {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        // Do not leave a failed-start loop running after reporting failure.
        self.stop()?;
        bail!(
            "Relay did not become ready; inspect removent-relay logs and macOS Login Items settings"
        )
    }
    fn stop(&self) -> Result<()> {
        if let Some(job) = self.job()? {
            self.checked(&["bootout", &self.target()])?;
            if let Some(pid) = job.pid {
                let deadline = Instant::now() + Duration::from_secs(15);
                while unsafe { libc::kill(pid as i32, 0) } == 0 {
                    ensure!(Instant::now() < deadline, "Relay is still shutting down");
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        if self.ready_file().exists() {
            fs::remove_file(self.ready_file())?;
        }
        Ok(())
    }
    pub(super) fn setup(&self, network: NetworkArgs, no_start: bool) -> Result<()> {
        self.require_session()?;
        if !self.config().exists() {
            initialize(&self.dir, network, None)?;
        } else {
            ensure!(
                network.address.is_none()
                    && network.listen.is_none()
                    && network.allowed_cidrs.is_empty()
                    && network.room == "office",
                "Already configured; edit server.toml, then check and restart. Setup never resets credentials."
            );
        }
        let _lock = self.lock()?;
        self.installed_login_file()?;
        let config = read_config(&self.config())?;
        ensure!(
            normalize(&config.identity_dir)? == self.dir.join("data"),
            "Managed relay identity must remain in its data directory"
        );
        let record_path = self.dir.join("service.json");
        let executable = fs::canonicalize(std::env::current_exe()?)?;
        if record_path.exists() {
            ensure!(
                self.installation()?.executable == executable,
                "Existing service uses a different executable; uninstall its service before reinstalling"
            );
        } else {
            write_new(
                &record_path,
                &serde_json::to_string_pretty(&Installation {
                    version: 1,
                    executable,
                })?,
            )?;
        }
        if self.job()?.is_none() {
            self.prepare_logs()?;
        }
        if !no_start {
            self.set_login(true)?;
            self.start()?;
        }
        println!(
            "macOS relay configured: {}\nAllow UDP {} through your firewall/router.\nUse removent-relay status and logs. Automatic startup is at user login; logout stops the relay.",
            self.dir.display(),
            config.listen.port()
        );
        Ok(())
    }
    pub(super) fn action(&self, action: &str) -> Result<()> {
        self.require_session()?;
        self.installation()?;
        if action == "status" {
            let job = self.job()?;
            println!(
                "{}",
                serde_json::json!({
                    "installed": true, "launch_at_login": self.installed_login_file()?,
                    "loaded": job.is_some(), "running": job.as_ref().is_some_and(|job| self.ready(job)),
                    "pid": job.and_then(|job| job.pid), "config": self.config(),
                })
            );
            return Ok(());
        }
        let _lock = self.lock()?;
        match action {
            "start" => self.start(),
            "stop" => self.stop(),
            "restart" => {
                read_config(&self.config())?;
                self.stop()?;
                self.start()
            }
            "enable" => self.set_login(true),
            "disable" => self.set_login(false),
            _ => bail!("Unknown service action"),
        }
    }
    pub(super) fn logs(&self, follow: bool, lines: u32) -> Result<()> {
        self.installation()?;
        let paths = [read_config(&self.config())?
            .identity_dir
            .join("logs/removent-relay.log")];
        for path in &paths {
            let meta = fs::symlink_metadata(path)?;
            ensure!(
                meta.is_file()
                    && meta.uid() == unsafe { libc::geteuid() }
                    && meta.mode() & 0o077 == 0,
                "Unsafe relay log file"
            );
        }
        let mut command = Command::new("/usr/bin/tail");
        command.args(["-n", &lines.to_string()]);
        if follow {
            command.arg("-F");
        }
        ensure!(
            command.args(paths).status()?.success(),
            "Cannot read relay logs"
        );
        Ok(())
    }
    pub(super) fn uninstall(&self) -> Result<()> {
        self.require_session()?;
        self.installation()?;
        let _lock = self.lock()?;
        self.installed_login_file()?;
        self.stop()?;
        self.set_login(false)?;
        fs::remove_file(self.dir.join("service.json"))?;
        if self.transient().exists() {
            fs::remove_file(self.transient())?;
        }
        println!("LaunchAgent removed. Binary, private configuration and identity retained.");
        Ok(())
    }
}

fn normalize(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(path.canonicalize()?);
    }
    let parent = path.parent().context("Invalid service directory")?;
    Ok(normalize(parent)?.join(path.file_name().context("Invalid service directory")?))
}
fn path_str(path: &Path) -> Result<&str> {
    path.to_str().context("Service paths must be valid UTF-8")
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut stage = tempfile::NamedTempFile::new_in(path.parent().context("Invalid output path")?)?;
    stage
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    stage.write_all(bytes)?;
    stage.as_file().sync_all()?;
    stage.persist(path)?;
    Ok(())
}
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
fn render_plist(label: &str, executable: &Path, dir: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
{MARKER}
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>{label}</string>
<key>ProgramArguments</key><array><string>{executable}</string><string>serve</string><string>{dir}/server.toml</string></array>
<key>EnvironmentVariables</key><dict><key>REMOVENT_RELAY_READY_FILE</key><string>{dir}/run/ready.pid</string></dict>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><true/>
<key>ThrottleInterval</key><integer>5</integer>
<key>ExitTimeOut</key><integer>10</integer>
<key>Umask</key><integer>63</integer>
<key>StandardOutPath</key><string>{dir}/logs/relay.log</string>
<key>StandardErrorPath</key><string>{dir}/logs/relay.err.log</string>
<key>ProcessType</key><string>Background</string>
</dict></plist>
"#,
        label = xml(label),
        executable = xml(&executable.to_string_lossy()),
        dir = xml(&dir.to_string_lossy())
    )
}

pub(super) fn notify_ready() -> Result<ReadyGuard> {
    let path = std::env::var_os("REMOVENT_RELAY_READY_FILE").map(PathBuf::from);
    if let Some(path) = &path {
        ensure!(path.is_absolute(), "Readiness path must be absolute");
        private_dir(path.parent().context("Invalid readiness path")?)?;
        atomic_write(path, std::process::id().to_string().as_bytes())?;
    }
    Ok(ReadyGuard { path })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plist_escapes_paths_and_keeps_credentials_out_of_arguments() {
        let body = render_plist(
            LABEL,
            Path::new("/Applications/a & b/relay"),
            Path::new("/Users/test/a < b"),
        );
        assert!(body.contains("a &amp; b/relay"));
        assert!(body.contains("a &lt; b/server.toml"));
        assert!(body.contains("<key>Umask</key><integer>63</integer>"));
        assert!(!body.contains("/bin/sh"));
        let temp = tempfile::NamedTempFile::new().unwrap();
        fs::write(temp.path(), body).unwrap();
        assert!(
            Command::new("/usr/bin/plutil")
                .arg("-lint")
                .arg(temp.path())
                .status()
                .unwrap()
                .success()
        );
    }
    #[test]
    fn custom_instance_label_is_stable_before_and_after_creation() {
        let temp = tempfile::tempdir_in("/tmp").unwrap();
        let dir = temp.path().join("relay");
        let first = Service::new(Some(&dir)).unwrap();
        fs::create_dir(&dir).unwrap();
        let second = Service::new(Some(&dir)).unwrap();
        assert_eq!(first.label, second.label);
        assert_ne!(first.label, LABEL);
    }
}

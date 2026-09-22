//! Linux systemd service backend.
use super::*;
use std::process::Command;

const CONFIG_DIR: &str = "/etc/removent-relay";
const STATE_DIR: &str = "/var/lib/removent-relay";
const BINARY: &str = "/usr/local/bin/removent-relay";
const UNIT: &str = "removent-relay.service";
const UNIT_PATH: &str = "/etc/systemd/system/removent-relay.service";
const UNIT_MARKER: &str = "# Managed by removent-relay setup";

fn linux() -> Result<()> {
    ensure!(
        cfg!(target_os = "linux"),
        "Service management requires Linux with systemd. Use serve CONFIG for foreground operation."
    );
    ensure!(
        Path::new("/run/systemd/system").is_dir(),
        "A running systemd manager is required"
    );
    Ok(())
}
fn root() -> Result<()> {
    ensure!(
        unsafe { libc::geteuid() } == 0,
        "Run this service-management command with sudo"
    );
    Ok(())
}
fn run(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("Cannot run {program}"))?;
    ensure!(status.success(), "{program} failed with status {status}");
    Ok(())
}
fn managed_unit() -> Result<()> {
    let unit = read_file(Path::new(UNIT_PATH), 16384, false)
        .context("Service not installed; run sudo removent-relay setup")?;
    ensure!(
        unit.starts_with(UNIT_MARKER),
        "Existing systemd unit was not installed by this CLI; refusing to modify it"
    );
    Ok(())
}

fn service_lock() -> Result<File> {
    use std::os::fd::AsRawFd;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open("/run/removent-relay-management.lock")?;
    let meta = file.metadata()?;
    ensure!(
        meta.is_file() && meta.uid() == 0 && meta.mode() & 0o077 == 0,
        "Unsafe service management lock"
    );
    ensure!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "Another relay management command is in progress; retry when it finishes"
    );
    Ok(file)
}

pub fn setup(network: NetworkArgs, no_start: bool) -> Result<()> {
    linux()?;
    root()?;
    let _lock = service_lock()?;
    let executable = fs::canonicalize(std::env::current_exe()?)?;
    ensure!(
        executable == Path::new(BINARY),
        "Install the binary to /usr/local/bin/removent-relay before setup"
    );
    let meta = fs::metadata(&executable)?;
    ensure!(
        meta.uid() == 0 && meta.mode() & 0o022 == 0,
        "Service binary must be root-owned and not group/world writable"
    );
    // LoadCredential is available in systemd 247+. It keeps /etc and exported
    // credentials root-only, while the daemon runs as an unprivileged DynamicUser.
    let version = Command::new("systemctl").arg("--version").output()?;
    ensure!(version.status.success(), "Cannot query systemd version");
    let version: u32 = String::from_utf8_lossy(&version.stdout)
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .context("Cannot read systemd version")?;
    ensure!(version >= 247, "systemd 247 or later is required");
    if Path::new(UNIT_PATH).exists() {
        managed_unit()?;
    }
    let config = Path::new(CONFIG_DIR).join("server.toml");
    if !config.exists() {
        initialize(Path::new(CONFIG_DIR), network, Some(Path::new(STATE_DIR)))?;
    } else {
        ensure!(
            network.address.is_none()
                && network.listen.is_none()
                && network.allowed_cidrs.is_empty()
                && network.room == "office",
            "Already configured; edit /etc/removent-relay/server.toml, then check and restart. Setup never resets credentials."
        );
        private_dir(Path::new(CONFIG_DIR))?;
    }
    let cfg = read_config(&config)?;
    ensure!(
        cfg.identity_dir == Path::new(STATE_DIR),
        "Managed service requires identity_dir = /var/lib/removent-relay"
    );
    let unit = service_unit();
    let mut stage = tempfile::NamedTempFile::new_in("/etc/systemd/system")?;
    stage
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o644))?;
    stage.write_all(unit.as_bytes())?;
    stage.as_file().sync_all()?;
    stage.persist(UNIT_PATH)?;
    run("systemctl", &["daemon-reload"])?;
    if !no_start {
        run("systemctl", &["enable", UNIT])?;
        service_inner("start")?;
    }
    println!(
        "Service installed. Allow UDP {} in your VPS firewall.\nUse removent-relay status and removent-relay logs to inspect it.",
        cfg.listen.port()
    );
    Ok(())
}

pub(super) fn service_unit() -> String {
    format!(
        "{UNIT_MARKER}\n{}",
        include_str!("../../../../deploy/relay/removent-relay.service")
    )
}

pub fn service(action: &str) -> Result<()> {
    linux()?;
    ensure!(
        ["start", "stop", "restart", "status", "enable", "disable"].contains(&action),
        "Unknown service action"
    );
    let _lock = if action != "status" {
        root()?;
        Some(service_lock()?)
    } else {
        None
    };
    service_inner(action)
}

fn service_inner(action: &str) -> Result<()> {
    managed_unit()?;
    if action == "status" {
        return run(
            "systemctl",
            &[
                "show",
                UNIT,
                "--no-pager",
                "--property=LoadState,ActiveState,SubState,MainPID,UnitFileState",
            ],
        );
    }
    if matches!(action, "start" | "restart") {
        read_config(&Path::new(CONFIG_DIR).join("server.toml"))?;
    }
    run("systemctl", &[action, UNIT])?;
    if matches!(action, "start" | "restart") {
        // Type=notify waits for the bound transport, so a failed bind cannot
        // be reported as a successful start.
        let active = Command::new("systemctl")
            .args(["is-active", "--quiet", UNIT])
            .status()?;
        ensure!(
            active.success(),
            "Relay did not stay running; inspect removent-relay logs"
        );
    }
    Ok(())
}

pub fn logs(follow: bool, lines: u32) -> Result<()> {
    linux()?;
    managed_unit()?;
    let lines = lines.to_string();
    let mut args = vec!["--unit", UNIT, "--lines", &lines, "--no-pager"];
    if follow {
        args.push("--follow");
    }
    run("journalctl", &args)
}

pub fn uninstall() -> Result<()> {
    linux()?;
    root()?;
    let _lock = service_lock()?;
    managed_unit()?;
    run("systemctl", &["disable", "--now", UNIT])?;
    fs::remove_file(UNIT_PATH)?;
    run("systemctl", &["daemon-reload"])?;
    println!("Service removed. Binary, private configuration and device identity retained.");
    Ok(())
}

pub fn notify_ready() -> Result<()> {
    #[cfg(target_os = "linux")]
    if let Some(path) = std::env::var_os("NOTIFY_SOCKET") {
        use std::os::{
            linux::net::SocketAddrExt,
            unix::{
                ffi::OsStrExt,
                net::{SocketAddr, UnixDatagram},
            },
        };
        let bytes = path.as_bytes();
        let address = if bytes.first() == Some(&b'@') {
            SocketAddr::from_abstract_name(&bytes[1..])?
        } else {
            SocketAddr::from_pathname(path)?
        };
        let socket = UnixDatagram::unbound()?;
        socket.connect_addr(&address)?;
        socket.send(b"READY=1")?;
    }
    Ok(())
}

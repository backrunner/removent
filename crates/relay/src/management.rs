//! Shared configuration and platform-specific service management. No shell interprets arguments.
use crate::cli::{NetworkArgs, ProfileRole};
use anyhow::{Context, Result, ensure};
use removent_core::{
    DataPaths,
    removent_uri::{RelayTransport, RemoventEndpoint},
};
use removent_relay::config::{Room, ServerConfig, TunnelConfig, decode_secret, token_hash};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{IsTerminal, Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

pub fn default_config_dir(service_dir: Option<&Path>) -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        macos::config_dir(service_dir)
    }
    #[cfg(not(target_os = "macos"))]
    {
        ensure!(
            service_dir.is_none(),
            "--service-dir is only available on macOS"
        );
        Ok(PathBuf::from("/etc/removent-relay"))
    }
}

pub fn setup(network: NetworkArgs, no_start: bool, service_dir: Option<&Path>) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos::Service::new(service_dir)?.setup(network, no_start)
    }
    #[cfg(target_os = "linux")]
    {
        default_config_dir(service_dir)?;
        linux::setup(network, no_start)
    }
}

pub fn service(action: &str, service_dir: Option<&Path>) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos::Service::new(service_dir)?.action(action)
    }
    #[cfg(target_os = "linux")]
    {
        default_config_dir(service_dir)?;
        linux::service(action)
    }
}

pub fn logs(follow: bool, lines: u32, service_dir: Option<&Path>) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos::Service::new(service_dir)?.logs(follow, lines)
    }
    #[cfg(target_os = "linux")]
    {
        default_config_dir(service_dir)?;
        linux::logs(follow, lines)
    }
}

pub fn uninstall(service_dir: Option<&Path>) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        macos::Service::new(service_dir)?.uninstall()
    }
    #[cfg(target_os = "linux")]
    {
        default_config_dir(service_dir)?;
        linux::uninstall()
    }
}

pub struct ReadyGuard {
    #[cfg(target_os = "macos")]
    path: Option<PathBuf>,
}

pub fn notify_ready() -> Result<ReadyGuard> {
    #[cfg(target_os = "linux")]
    {
        linux::notify_ready()?;
        Ok(ReadyGuard {})
    }
    #[cfg(target_os = "macos")]
    {
        macos::notify_ready()
    }
}

impl Drop for ReadyGuard {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(path) = &self.path
            && read_private(path, 32).ok().as_deref() == Some(&std::process::id().to_string())
        {
            let _ = fs::remove_file(path);
        }
    }
}

pub fn user_config_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .context("Set HOME or pass an explicit configuration directory")?;
    ensure!(base.is_absolute(), "Configuration base must be absolute");
    Ok(base.join("removent-relay"))
}

fn read_file(path: &Path, limit: u64, private: bool) -> Result<String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .with_context(|| format!("Cannot open {}", path.display()))?;
    let meta = file.metadata()?;
    ensure!(meta.is_file(), "Expected a regular file");
    if private {
        ensure!(
            meta.mode() & 0o077 == 0 && meta.uid() == unsafe { libc::geteuid() },
            "Credentials must be owned by the current user and private (chmod 600)"
        );
    }
    ensure!(meta.len() <= limit, "Configuration file is too large");
    let mut value = String::new();
    file.take(limit + 1).read_to_string(&mut value)?;
    ensure!(
        value.len() as u64 <= limit,
        "Configuration file is too large"
    );
    Ok(value)
}
pub fn read_private(path: &Path, limit: u64) -> Result<String> {
    read_file(path, limit, true)
}
pub fn read_config(path: &Path) -> Result<ServerConfig> {
    let config: ServerConfig = toml::from_str(&read_file(path, 1024 * 1024, false)?)
        .map_err(|_| anyhow::anyhow!("Invalid relay server configuration (contents redacted)"))?;
    config.validate()?;
    ensure!(
        config.identity_dir.is_absolute(),
        "identity_dir must be an absolute path"
    );
    Ok(config)
}

fn write_new(path: &Path, body: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| {
            format!(
                "Cannot create {} (existing files are never overwritten)",
                path.display()
            )
        })?;
    file.write_all(body.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

fn private_dir(path: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_dir() && meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0,
        "Configuration directory must be a private, owned directory (chmod 700), not a symlink"
    );
    Ok(())
}

fn prompt(label: &str, default: &str) -> Result<String> {
    ensure!(
        std::io::stdin().is_terminal(),
        "Noninteractive setup requires --address removent://host:port"
    );
    print!(
        "{label}{}: ",
        if default.is_empty() {
            String::new()
        } else {
            format!(" [{default}]")
        }
    );
    std::io::stdout().flush()?;
    let mut value = String::new();
    ensure!(
        std::io::stdin().read_line(&mut value)? != 0,
        "Setup input closed"
    );
    Ok(if value.trim().is_empty() {
        default.to_string()
    } else {
        value.trim().to_string()
    })
}

fn resolve_network(mut args: NetworkArgs) -> Result<(RemoventEndpoint, NetworkArgs)> {
    if args.address.is_none() {
        args.address = Some(prompt("Public relay address (removent://host:port)", "")?);
        args.room = prompt("Room", &args.room)?;
        if args.allowed_cidrs.is_empty() {
            let cidrs = prompt(
                "Allowed source CIDRs, comma separated (blank = unrestricted)",
                "",
            )?;
            for value in cidrs.split(',').filter(|v| !v.trim().is_empty()) {
                args.allowed_cidrs
                    .push(value.trim().parse().context("Invalid source CIDR")?);
            }
        }
    }
    let endpoint = RemoventEndpoint::parse(args.address.as_deref().unwrap_or_default())
        .map_err(anyhow::Error::msg)?;
    ensure!(
        removent_relay::config::valid_room(&args.room),
        "Invalid room name"
    );
    Ok((endpoint, args))
}

/// Stage all files before making the configuration visible. Existing identities
/// and credentials are never regenerated by setup, install or restart.
pub fn initialize(dir: &Path, args: NetworkArgs, state_dir: Option<&Path>) -> Result<()> {
    let dir = std::path::absolute(dir)?;
    ensure!(
        !dir.exists() && !dir.is_symlink(),
        "Already configured; edit server.toml and use check/restart. Existing credentials are preserved."
    );
    let parent = dir
        .parent()
        .context("Configuration directory needs a parent")?;
    for ancestor in parent.ancestors() {
        ensure!(
            !ancestor.join(".git").exists(),
            "Store private relay configuration outside a source checkout"
        );
    }
    let (endpoint, args) = resolve_network(args)?;
    let listen = args
        .listen
        .unwrap_or_else(|| std::net::SocketAddr::from(([0, 0, 0, 0], endpoint.port)));
    ensure!(listen.port() != 0, "Use a nonzero listening port");
    // Validate public settings before creating any files or identity.
    let host_token = hex::encode(rand::random::<[u8; 32]>());
    let mut client_token = hex::encode(rand::random::<[u8; 32]>());
    while host_token == client_token {
        client_token = hex::encode(rand::random::<[u8; 32]>());
    }
    let identity_dir = state_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| dir.join("data"));
    let config = ServerConfig {
        listen,
        identity_dir,
        max_connections: 128,
        max_clients_per_room: 4,
        max_bytes_per_second: 50_000_000,
        allowed_cidrs: args.allowed_cidrs,
        rooms: vec![Room {
            name: args.room.clone(),
            host_token_sha256: hex::encode(token_hash(&host_token)?),
            client_token_sha256: hex::encode(token_hash(&client_token)?),
            host_public_keys: vec![],
            client_public_keys: vec![],
        }],
    };
    config.validate()?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    let stage = tempfile::Builder::new()
        .prefix(".removent-relay-")
        .tempdir_in(parent)?;
    fs::set_permissions(stage.path(), fs::Permissions::from_mode(0o700))?;
    let actual_state = state_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| stage.path().join("data"));
    private_dir(&actual_state)?;
    let identity = removent_core::identity::load_or_create(
        &DataPaths { root: actual_state },
        "Removent relay",
    )?;
    write_new(
        &stage.path().join("server.toml"),
        &toml::to_string_pretty(&config)?,
    )?;
    for (role, token) in [("host", host_token), ("client", client_token)] {
        let profile = TunnelConfig {
            server: endpoint.uri(),
            transport: RelayTransport::Quic,
            insecure_loopback: false,
            server_fingerprint: identity.fingerprint_hex(),
            host_fingerprint: String::new(),
            room: args.room.clone(),
            token,
        };
        profile.validate()?;
        write_new(
            &stage.path().join(format!("relay-{role}.toml")),
            &toml::to_string_pretty(&profile)?,
        )?;
    }
    File::open(stage.path())?.sync_all()?;
    fs::rename(stage.path(), &dir).context("Cannot publish relay configuration")?;
    File::open(parent)?.sync_all()?;
    println!(
        "Configuration: {}\nRelay fingerprint: {}\nPrivate profiles: relay-host.toml and relay-client.toml\nSet the controller profile's host_fingerprint to the target Mac's verified identity.",
        dir.display(),
        identity.fingerprint_hex()
    );
    Ok(())
}

pub fn fingerprint(config: &ServerConfig) -> Result<String> {
    let cert = config.identity_dir.join("identity/device.crt");
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(cert)
        .context("Relay identity not found; run setup or init first")?;
    ensure!(
        file.metadata()?.is_file() && file.metadata()?.len() <= 65536,
        "Invalid relay certificate file"
    );
    let mut bytes = vec![];
    file.take(65537).read_to_end(&mut bytes)?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= 65536,
        "Invalid relay certificate"
    );
    Ok(hex::encode(Sha256::digest(bytes)))
}

pub fn export_profile(
    role: ProfileRole,
    dir: &Path,
    output: &Path,
    host: Option<&str>,
) -> Result<()> {
    let mut profile: TunnelConfig = toml::from_str(&read_private(
        &dir.join(format!("relay-{}.toml", role.name())),
        16384,
    )?)
    .map_err(|_| anyhow::anyhow!("Invalid saved profile (contents redacted)"))?;
    if let Some(host) = host {
        decode_secret(host)?;
        profile.host_fingerprint = host.to_string();
    }
    let config = read_config(&dir.join("server.toml"))?;
    profile.server_fingerprint = fingerprint(&config)?;
    let room = config
        .rooms
        .iter()
        .find(|r| r.name == profile.room)
        .context("Saved profile room is no longer configured")?;
    let expected = match role {
        ProfileRole::Host => &room.host_token_sha256,
        ProfileRole::Client => &room.client_token_sha256,
    };
    ensure!(
        token_hash(&profile.token)? == decode_secret(expected)?,
        "Saved profile credential no longer matches server.toml; use the updated credential"
    );
    profile.validate()?;
    let parent = std::path::absolute(output)?
        .parent()
        .context("Output needs a parent directory")?
        .to_path_buf();
    ensure!(parent.is_dir(), "Output parent directory does not exist");
    let mut stage = tempfile::NamedTempFile::new_in(parent)?;
    stage
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    stage.write_all(toml::to_string_pretty(&profile)?.as_bytes())?;
    stage.as_file().sync_all()?;
    stage
        .persist_noclobber(output)
        .context("Output already exists or could not be written")?;
    println!(
        "Private {} profile written to {}",
        role.name(),
        output.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn network() -> NetworkArgs {
        NetworkArgs {
            address: Some("removent://relay.example:48700".into()),
            room: "office".into(),
            listen: None,
            allowed_cidrs: vec![
                "203.0.113.0/24".parse().unwrap(),
                "2001:db8::/32".parse().unwrap(),
            ],
        }
    }
    #[test]
    fn init_profiles_permissions_identity_and_no_clobber() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("private relay");
        initialize(&dir, network(), None).unwrap();
        let cfg = read_config(&dir.join("server.toml")).unwrap();
        let host = TunnelConfig::load(&dir.join("relay-host.toml")).unwrap();
        let client = TunnelConfig::load(&dir.join("relay-client.toml")).unwrap();
        assert_eq!(host.server_fingerprint, fingerprint(&cfg).unwrap());
        assert_eq!(host.server_fingerprint, client.server_fingerprint);
        assert_ne!(host.token, client.token);
        assert_eq!(
            hex::encode(token_hash(&host.token).unwrap()),
            cfg.rooms[0].host_token_sha256
        );
        assert_eq!(cfg.allowed_cidrs.len(), 2);
        assert!(
            !fs::read_to_string(dir.join("server.toml"))
                .unwrap()
                .contains(&host.token)
        );
        assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        for name in ["server.toml", "relay-host.toml", "relay-client.toml"] {
            assert_eq!(fs::metadata(dir.join(name)).unwrap().mode() & 0o777, 0o600);
        }
        assert!(initialize(&dir, network(), None).is_err());
        assert_eq!(
            host.token,
            TunnelConfig::load(&dir.join("relay-host.toml"))
                .unwrap()
                .token
        );
        let output = temp.path().join("exported.toml");
        export_profile(ProfileRole::Client, &dir, &output, Some(&"ab".repeat(32))).unwrap();
        assert_eq!(
            TunnelConfig::load(&output).unwrap().host_fingerprint,
            "ab".repeat(32)
        );
        assert!(export_profile(ProfileRole::Client, &dir, &output, None).is_err());
    }
    #[test]
    fn private_files_reject_symlinks_public_modes_and_oversized_input() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("token");
        write_new(&file, &"11".repeat(32)).unwrap();
        assert!(read_private(&file, 128).is_ok());
        assert!(read_private(&file, 20).is_err());
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(read_private(&link, 128).is_err());
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_private(&file, 128).is_err());
    }
    #[test]
    fn malformed_config_does_not_echo_secrets_or_create_a_directory() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("new");
        let mut args = network();
        args.room = "bad room".into();
        assert!(initialize(&dir, args, None).is_err());
        assert!(!dir.exists());
        let config = temp.path().join("server.toml");
        write_new(&config, "SECRET_NOT_TO_LOG invalid=").unwrap();
        let error = format!("{:#}", read_config(&config).err().unwrap());
        assert!(!error.contains("SECRET_NOT_TO_LOG"));
    }
    #[test]
    #[cfg(target_os = "linux")]
    fn unit_uses_credential_delivery_and_unprivileged_persistent_state() {
        let unit = linux::service_unit();
        assert!(unit.contains("DynamicUser=yes"));
        assert!(unit.contains("LoadCredential=server.toml:/etc/removent-relay/server.toml"));
        assert!(unit.contains("serve %d/server.toml"));
        assert!(unit.contains("StateDirectory=removent-relay"));
        assert!(!unit.contains("/bin/bash"));
    }
}

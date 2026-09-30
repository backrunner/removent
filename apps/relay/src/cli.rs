//! User-facing CLI. Serving and service management share one installed binary.
use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use removent_relay::config::ServerConfig;
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(
    name = "removent-relay",
    version,
    about = "Private Removent relay and service manager"
)]
struct Cli {
    /// Separate macOS relay configuration and launchd instance (advanced).
    #[arg(long, global = true)]
    service_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
pub struct NetworkArgs {
    /// Public address advertised to Macs, e.g. removent://relay.example.com:48700.
    #[arg(long)]
    pub address: Option<String>,
    #[arg(long, default_value = "office")]
    pub room: String,
    /// Bind address. Defaults to 0.0.0.0 with the public address's port.
    #[arg(long)]
    pub listen: Option<SocketAddr>,
    /// Allowed source networks; repeat or separate by commas. Empty = any source.
    #[arg(long = "allow-cidr", value_delimiter = ',')]
    pub allowed_cidrs: Vec<ipnet::IpNet>,
}

#[derive(Subcommand)]
enum Command {
    /// Configure a Linux systemd service (sudo) or macOS user LaunchAgent (no sudo).
    Setup {
        #[command(flatten)]
        network: NetworkArgs,
        /// Install without enabling automatic startup or starting the service.
        #[arg(long)]
        no_start: bool,
    },
    /// Generate private configuration and independent host/client profiles.
    Init {
        #[arg(long)]
        dir: Option<PathBuf>,
        #[command(flatten)]
        network: NetworkArgs,
    },
    /// Validate a server configuration without starting or changing anything.
    Check {
        #[arg(help = "Server config; defaults to the installed service configuration")]
        config: Option<PathBuf>,
    },
    /// Run the native QUIC relay in the foreground.
    Serve {
        #[arg(help = "Server config; defaults to the installed service configuration")]
        config: Option<PathBuf>,
    },
    /// Run a WebSocket relay behind an HTTPS ingress.
    ServeWebsocket { config: PathBuf },
    /// Run the Cloudflare container using REMOVENT_RELAY_CONFIG_JSON.
    ServeContainer,
    /// Start the installed service.
    Start,
    /// Stop the installed service; use disable to also prevent automatic startup.
    Stop,
    /// Validate configuration and restart the service.
    Restart,
    /// Show installation, automatic startup, process and service state.
    Status,
    /// Read the installed service's logs.
    Logs {
        #[arg(short, long)]
        follow: bool,
        #[arg(short = 'n', long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=10000))]
        lines: u32,
    },
    /// Enable automatic startup without changing the current running state.
    Enable,
    /// Disable automatic startup without stopping the currently running service.
    Disable,
    /// Remove the managed service, retaining the binary, configuration and identity.
    Uninstall,
    /// Print the existing relay certificate fingerprint; never creates an identity.
    Fingerprint {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Export a private host or client profile; refuses to overwrite existing files.
    Export {
        #[arg(value_enum)]
        role: ProfileRole,
        #[arg(long)]
        config_dir: Option<PathBuf>,
        #[arg(long)]
        output: PathBuf,
        /// Target Mac fingerprint for a controller profile, obtained via a trusted channel.
        #[arg(long)]
        host_fingerprint: Option<String>,
    },
    /// Check signed stable releases without downloading or installing an update.
    CheckUpdate {
        /// Read the active service version from this server configuration.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Generate a fresh credential and its SHA-256 hash for manual configuration.
    GenerateToken,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum ProfileRole {
    Host,
    Client,
}
impl ProfileRole {
    pub fn name(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Client => "client",
        }
    }
}
pub async fn execute() -> Result<Option<(ServerConfig, bool, bool)>> {
    use Command::*;
    let cli = Cli::parse();
    let service_dir = cli.service_dir.as_deref();
    let config_dir = || crate::management::default_config_dir(service_dir);
    let config_path = |path: Option<PathBuf>| -> Result<PathBuf> {
        path.map(Ok)
            .unwrap_or_else(|| Ok(config_dir()?.join("server.toml")))
    };
    match cli.command {
        Serve { config } => {
            return Ok(Some((
                crate::management::read_config(&config_path(config)?)?,
                false,
                false,
            )));
        }
        ServeWebsocket { config } => {
            return Ok(Some((
                crate::management::read_config(&config)?,
                true,
                false,
            )));
        }
        ServeContainer => {
            let config: ServerConfig = serde_json::from_str(
                &std::env::var("REMOVENT_RELAY_CONFIG_JSON")
                    .context("Missing container relay configuration")?,
            )
            .map_err(|_| {
                anyhow::anyhow!("Invalid container relay configuration (contents redacted)")
            })?;
            config.validate()?;
            return Ok(Some((config, true, true)));
        }
        Setup { network, no_start } => crate::management::setup(network, no_start, service_dir)?,
        Init { dir, network } => {
            let dir = dir
                .map(Ok)
                .unwrap_or_else(crate::management::user_config_dir)?;
            crate::management::initialize(&dir, network, None)?;
        }
        Check { config } => {
            let config = config_path(config)?;
            crate::management::read_config(&config)?;
            println!("Configuration valid: {}", config.display());
        }
        Start => crate::management::service("start", service_dir)?,
        Stop => crate::management::service("stop", service_dir)?,
        Restart => crate::management::service("restart", service_dir)?,
        Status => crate::management::service("status", service_dir)?,
        Enable => crate::management::service("enable", service_dir)?,
        Disable => crate::management::service("disable", service_dir)?,
        Uninstall => crate::management::uninstall(service_dir)?,
        Logs { follow, lines } => crate::management::logs(follow, lines, service_dir)?,
        Fingerprint { config } => println!(
            "{}",
            crate::management::fingerprint(&crate::management::read_config(&config_path(
                config
            )?)?)?
        ),
        Export {
            role,
            config_dir,
            output,
            host_fingerprint,
        } => crate::management::export_profile(
            role,
            &config_dir
                .map(Ok)
                .unwrap_or_else(|| crate::management::default_config_dir(service_dir))?,
            &output,
            host_fingerprint.as_deref(),
        )?,
        CheckUpdate { config } => {
            let path = config.or_else(|| {
                config_dir()
                    .ok()
                    .map(|dir| dir.join("server.toml"))
                    .filter(|p| p.exists())
            });
            let config = path
                .map(|path| crate::management::read_config(&path))
                .transpose()?;
            crate::updater::check_command(config.as_ref()).await?;
        }
        GenerateToken => {
            let token = hex::encode(rand::random::<[u8; 32]>());
            println!(
                "token = \"{token}\"\nsha256 = \"{}\"",
                hex::encode(removent_relay::config::token_hash(&token)?)
            );
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cli_requires_explicit_commands_and_valid_options() {
        assert!(Cli::try_parse_from(["removent-relay"]).is_err());
        assert!(Cli::try_parse_from(["removent-relay", "stop", "--unknown"]).is_err());
        assert!(
            Cli::try_parse_from(["removent-relay", "setup", "--allow-cidr", "1.2.3.4/33"]).is_err()
        );
        assert!(Cli::try_parse_from(["removent-relay", "logs", "--lines", "0"]).is_err());
        assert!(Cli::try_parse_from(["removent-relay", "serve", "/tmp/server.toml"]).is_ok());
        assert!(Cli::try_parse_from(["removent-relay", "serve-container"]).is_ok());
        assert!(Cli::try_parse_from(["removent-relay", "check-update"]).is_ok());
        assert!(Cli::try_parse_from(["removent-relay", "cloudflare", "status"]).is_err());
    }
}

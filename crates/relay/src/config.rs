use anyhow::{Context, Result, ensure};
pub use removent_core::removent_uri::valid_room;
use removent_core::removent_uri::{RelayTransport, RemoventEndpoint};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, net::SocketAddr, path::Path};

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Host,
    Client,
}

/// Deliberately no Debug: this struct contains a bearer credential.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TunnelConfig {
    pub server: String,
    pub transport: RelayTransport,
    /// In-process loopback test harness only; never accepted from a profile.
    #[serde(skip)]
    pub insecure_loopback: bool,
    #[serde(default)]
    pub server_fingerprint: String,
    /// Target RVP certificate pin used by desktop/CLI, never sent to the relay.
    #[serde(default)]
    pub host_fingerprint: String,
    pub room: String,
    #[serde(default)]
    pub token: String,
}

impl TunnelConfig {
    pub fn load(path: &Path) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                std::fs::metadata(path)?.permissions().mode() & 0o077 == 0,
                "relay credentials file must be private (chmod 600)"
            );
        }
        let cfg: Self = toml::from_str(&std::fs::read_to_string(path)?)
            .map_err(|_| anyhow::anyhow!("Invalid relay configuration (contents redacted)"))?;
        cfg.validate()?;
        Ok(cfg)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(valid_room(&self.room), "Invalid relay room");
        if !self.token.is_empty() {
            decode_secret(&self.token)?;
        }
        let endpoint = self.endpoint()?;
        ensure!(
            !self.insecure_loopback || (self.is_websocket() && endpoint.is_loopback()),
            "Plaintext test transport requires loopback"
        );
        if self.is_websocket() {
            ensure!(
                self.server_fingerprint.is_empty(),
                "WebSocket uses WebPKI; omit the QUIC server fingerprint"
            );
        } else {
            decode_secret(&self.server_fingerprint)?;
        }
        Ok(())
    }
    pub fn endpoint(&self) -> Result<RemoventEndpoint> {
        RemoventEndpoint::parse(&self.server).map_err(anyhow::Error::msg)
    }
    pub fn websocket_url(&self) -> Result<String> {
        self.validate()?;
        let url = self.endpoint()?.websocket_url();
        Ok(if self.insecure_loopback {
            url.replacen("wss://", "ws://", 1)
        } else {
            url
        })
    }
    pub fn is_websocket(&self) -> bool {
        self.transport == RelayTransport::WebSocket
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Room {
    pub name: String,
    #[serde(default)]
    pub host_token_sha256: String,
    #[serde(default)]
    pub client_token_sha256: String,
    #[serde(default)]
    pub host_public_keys: Vec<String>,
    #[serde(default)]
    pub client_public_keys: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    pub identity_dir: std::path::PathBuf,
    pub max_connections: usize,
    pub max_clients_per_room: usize,
    /// Per authenticated connection, in each ingress direction; burst is one second.
    pub max_bytes_per_second: u64,
    /// Empty means unrestricted; matched against the actual socket peer IP.
    #[serde(default)]
    pub allowed_cidrs: Vec<ipnet::IpNet>,
    pub rooms: Vec<Room>,
}

impl ServerConfig {
    pub fn allows_ip(&self, ip: std::net::IpAddr) -> bool {
        let ip = match ip {
            std::net::IpAddr::V6(v6) => v6.to_ipv4_mapped().map(std::net::IpAddr::V4).unwrap_or(ip),
            ip => ip,
        };
        self.allowed_cidrs.is_empty()
            || self.allowed_cidrs.iter().any(|net| {
                if let ipnet::IpNet::V6(v6) = net
                    && let Some(v4) = v6.addr().to_ipv4_mapped()
                    && v6.prefix_len() >= 96
                {
                    return ipnet::Ipv4Net::new(v4, v6.prefix_len() - 96)
                        .unwrap()
                        .contains(&match ip {
                            std::net::IpAddr::V4(v4) => v4,
                            _ => return false,
                        });
                }
                net.contains(&ip)
            })
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(self.allowed_cidrs.len() <= 256, "At most 256 allowed CIDRs");
        ensure!(self.allowed_cidrs.iter().all(|net| !matches!(net, ipnet::IpNet::V6(v6) if v6.addr().to_ipv4_mapped().is_some() && v6.prefix_len() < 96)), "Mapped IPv4 CIDRs require prefix 96 or greater");
        ensure!(
            (1..=4096).contains(&self.max_connections),
            "max_connections must be 1..4096"
        );
        ensure!(
            (1..=64).contains(&self.max_clients_per_room),
            "max_clients_per_room must be 1..64"
        );
        ensure!(
            (2048..=10_000_000_000).contains(&self.max_bytes_per_second),
            "Invalid bandwidth limit"
        );
        ensure!(
            !self.rooms.is_empty() && self.rooms.len() <= 4096,
            "Configure 1..4096 rooms"
        );
        let mut names = HashSet::new();
        for room in &self.rooms {
            ensure!(
                valid_room(&room.name) && names.insert(&room.name),
                "Invalid or duplicate room"
            );
            crate::auth::Policy::new(&room.host_token_sha256, &room.host_public_keys)?;
            crate::auth::Policy::new(&room.client_token_sha256, &room.client_public_keys)?;
            ensure!(
                room.host_token_sha256.is_empty()
                    || room.client_token_sha256.is_empty()
                    || decode_secret(&room.host_token_sha256)?
                        != decode_secret(&room.client_token_sha256)?,
                "Host and controller credentials must differ"
            );
            ensure!(
                room.host_public_keys.iter().all(|h| !room
                    .client_public_keys
                    .iter()
                    .any(|c| c.eq_ignore_ascii_case(h))),
                "Host and controller device keys must differ"
            );
        }
        Ok(())
    }
}

pub fn decode_secret(value: &str) -> Result<[u8; 32]> {
    hex::decode(value)
        .ok()
        .and_then(|v| v.try_into().ok())
        .context("Expected 64 hexadecimal characters")
}

/// Tokens are 256-bit random values, so fast hashing is appropriate (not passwords).
pub fn token_hash(token: &str) -> Result<[u8; 32]> {
    Ok(Sha256::digest(decode_secret(token)?).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_ipv6_and_mapped_peers_have_the_same_network_policy() {
        let mut cfg = ServerConfig {
            listen: "127.0.0.1:48700".parse().unwrap(),
            identity_dir: "/tmp/unused".into(),
            max_connections: 128,
            max_clients_per_room: 4,
            max_bytes_per_second: 50000000,
            rooms: vec![],
            allowed_cidrs: vec![
                "203.0.113.0/24".parse().unwrap(),
                "2001:db8::/32".parse().unwrap(),
            ],
        };
        for ip in [
            "203.0.113.0",
            "203.0.113.255",
            "::ffff:203.0.113.4",
            "2001:db8:ffff::1",
        ] {
            assert!(cfg.allows_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["203.0.112.255", "203.0.114.0", "2001:db9::1", "127.0.0.1"] {
            assert!(!cfg.allows_ip(ip.parse().unwrap()), "{ip}");
        }
        cfg.allowed_cidrs = vec!["::ffff:203.0.113.4/128".parse().unwrap()];
        assert!(cfg.allows_ip("203.0.113.4".parse().unwrap()));
        assert!(!cfg.allows_ip("203.0.113.5".parse().unwrap()));
        cfg.allowed_cidrs.clear();
        assert!(cfg.allows_ip("::1".parse().unwrap()));
        for bad in ["1.2.3.4/33", "::/129", "1.2.3.4", "localhost/32"] {
            assert!(bad.parse::<ipnet::IpNet>().is_err());
        }
    }

    #[test]
    fn profiles_require_an_explicit_carrier_and_cannot_opt_out_of_tls() {
        let profile = r#"
server = "removent://127.0.0.1:443"
transport = "websocket"
room = "office"
"#;
        let config: TunnelConfig = toml::from_str(profile).unwrap();
        assert_eq!(
            config.websocket_url().unwrap(),
            "wss://127.0.0.1:443/v1/tunnel"
        );
        assert!(
            toml::from_str::<TunnelConfig>(&format!("{profile}insecure_loopback = true\n"))
                .is_err()
        );
        assert!(
            toml::from_str::<TunnelConfig>(&profile.replace("transport = \"websocket\"", ""))
                .is_err()
        );
    }
}

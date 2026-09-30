//! Explicit protocol selection and address validation shared with connection forms.

pub use removent_core::removent_uri::RelayTransport;
use removent_core::removent_uri::{RemoventEndpoint, valid_room};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionProtocol {
    Removent,
    Vnc,
    Rdp,
}

impl ConnectionProtocol {
    pub const ALL: [Self; 3] = [Self::Removent, Self::Vnc, Self::Rdp];

    pub fn label(self) -> &'static str {
        match self {
            Self::Removent => "Removent",
            Self::Vnc => "VNC / Apple Remote Desktop",
            Self::Rdp => "RDP",
        }
    }

    /// Compact name for list rows and metadata.
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Removent => "Removent",
            Self::Vnc => "VNC",
            Self::Rdp => "RDP",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Self::Removent => removent_proto::DEFAULT_PORT,
            Self::Vnc => 5900,
            Self::Rdp => 3389,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionAddress {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AddressError {
    #[error("Enter a valid IP address or hostname")]
    Host,
    #[error("Port must be between 1 and 65535")]
    Port,
}

impl ConnectionAddress {
    pub fn parse_native(host: &str, port: &str) -> Result<Self, AddressError> {
        if host.trim().starts_with("removent://") {
            let endpoint = RemoventEndpoint::parse(host).map_err(|_| AddressError::Host)?;
            return Ok(Self {
                host: endpoint.host,
                port: endpoint.port,
            });
        }
        Self::parse(host, port)
    }
    pub fn relay_room(room: &str) -> Result<Self, AddressError> {
        if !valid_room(room.trim()) {
            return Err(AddressError::Host);
        }
        Ok(Self {
            host: room.trim().to_owned(),
            port: 0,
        })
    }

    pub fn parse(host: &str, port: &str) -> Result<Self, AddressError> {
        let port = port
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|p| *p != 0)
            .ok_or(AddressError::Port)?;
        let host = host.trim();
        let unbracketed = host
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or(host);
        let host = if let Ok(ip) = unbracketed.parse::<IpAddr>() {
            ip.to_string()
        } else if let Some((ip, scope)) = unbracketed.split_once('%') {
            let ip = ip.parse::<Ipv6Addr>().map_err(|_| AddressError::Host)?;
            if scope.is_empty()
                || !scope
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
            {
                return Err(AddressError::Host);
            }
            format!("{ip}%{scope}")
        } else {
            if host.is_empty()
                || host
                    .chars()
                    .any(|c| c.is_whitespace() || ":/\\?#@[]%".contains(c))
            {
                return Err(AddressError::Host);
            }
            match url::Host::parse(host).map_err(|_| AddressError::Host)? {
                url::Host::Domain(name)
                    if name
                        .strip_suffix('.')
                        .unwrap_or(&name)
                        .split('.')
                        .all(|part| {
                            !part.is_empty()
                                && part.len() <= 63
                                && !part.starts_with('-')
                                && !part.ends_with('-')
                                && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
                        })
                        && name.len() <= 253 =>
                {
                    name
                }
                url::Host::Ipv4(ip) => ip.to_string(),
                _ => return Err(AddressError::Host),
            }
        };
        Ok(Self { host, port })
    }

    pub fn from_socket(addr: SocketAddr) -> Self {
        let host = match addr {
            SocketAddr::V6(v6) if v6.scope_id() != 0 => format!("{}%{}", v6.ip(), v6.scope_id()),
            _ => addr.ip().to_string(),
        };
        Self {
            host,
            port: addr.port(),
        }
    }
}

impl std::fmt::Display for ConnectionAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.port == 0 {
            write!(f, "{}", self.host)
        } else if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

/// Credentials are per connection and deliberately excluded from Debug and persistence.
/// For native relay connections `password` is the relay credential; native RVP
/// authentication always uses the device identity and pairing grants.
pub struct ConnectionRequest {
    pub protocol: ConnectionProtocol,
    pub address: ConnectionAddress,
    pub username: String,
    pub password: String,
    pub domain: String,
    pub accept_invalid_certificate: bool,
    pub relay: Option<RelayRoute>,
}

impl ConnectionRequest {
    pub fn native(addr: SocketAddr) -> Self {
        Self {
            protocol: ConnectionProtocol::Removent,
            address: ConnectionAddress::from_socket(addr),
            username: String::new(),
            password: String::new(),
            domain: String::new(),
            accept_invalid_certificate: false,
            relay: None,
        }
    }
}

/// Only public routing/trust data. Credentials belong in the Keychain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayRoute {
    pub endpoint: String,
    pub transport: RelayTransport,
    pub server_fingerprint: String,
    pub host_fingerprint: String,
}
impl RelayRoute {
    pub fn parse(
        endpoint: &str,
        transport: RelayTransport,
        server_fingerprint: &str,
        host_fingerprint: &str,
    ) -> Result<Self, &'static str> {
        let endpoint = RemoventEndpoint::parse(endpoint).map_err(|_| "connection.invalid_relay")?;
        let pin = |value: &str| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit());
        let server_fingerprint = server_fingerprint.trim().to_lowercase();
        let host_fingerprint = host_fingerprint.trim().to_lowercase();
        if !pin(&host_fingerprint) {
            return Err("connection.invalid_host_fingerprint");
        }
        match transport {
            RelayTransport::Quic if !pin(&server_fingerprint) => {
                return Err("connection.invalid_relay_pin");
            }
            RelayTransport::WebSocket if !server_fingerprint.is_empty() => {
                return Err("connection.invalid_relay");
            }
            _ => {}
        }
        Ok(Self {
            endpoint: endpoint.uri(),
            transport,
            server_fingerprint,
            host_fingerprint,
        })
    }
    pub fn host_pin(&self) -> [u8; 32] {
        // Called only after parse/validate at the connection boundary.
        hex::decode(&self.host_fingerprint)
            .unwrap()
            .try_into()
            .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removent_routes_require_the_selected_carriers_trust_and_reject_old_schemes() {
        let host = "aa".repeat(32);
        let relay = "bb".repeat(32);
        let route = RelayRoute::parse(
            "removent://RELAY.example:443",
            RelayTransport::WebSocket,
            "",
            &host,
        )
        .unwrap();
        assert_eq!(route.endpoint, "removent://relay.example:443");
        assert_eq!(route.host_pin(), [0xaa; 32]);
        assert!(
            RelayRoute::parse(
                "removent://[::1]:48700",
                RelayTransport::Quic,
                &relay,
                &host
            )
            .is_ok()
        );
        assert!(
            RelayRoute::parse(
                "removent://relay.example:443",
                RelayTransport::WebSocket,
                &relay,
                &host
            )
            .is_err()
        );
        assert!(
            RelayRoute::parse(
                "removent://relay.example:48700",
                RelayTransport::Quic,
                "",
                &host
            )
            .is_err()
        );
        for old in [
            "wss://relay.example:443",
            "quic://relay.example:48700",
            "relay://office",
        ] {
            assert!(RelayRoute::parse(old, RelayTransport::WebSocket, "", &host).is_err());
            assert!(ConnectionAddress::parse_native(old, "48688").is_err());
        }
        assert_eq!(
            ConnectionAddress::parse_native("removent://[::1]:48688", "0")
                .unwrap()
                .to_string(),
            "[::1]:48688"
        );
        assert_eq!(
            ConnectionAddress::relay_room("office-mac_1")
                .unwrap()
                .to_string(),
            "office-mac_1"
        );
        for invalid in ["relay://office", "../office", "office/path", ""] {
            assert!(ConnectionAddress::relay_room(invalid).is_err());
        }
    }

    #[test]
    fn protocol_defaults_and_custom_ports_are_independent() {
        assert_eq!(ConnectionProtocol::Vnc.default_port(), 5900);
        assert_eq!(ConnectionProtocol::Rdp.default_port(), 3389);
        assert_eq!(
            ConnectionAddress::parse("office-pc.local", "3390")
                .unwrap()
                .port,
            3390
        );
    }

    #[test]
    fn hostnames_and_both_ip_families() {
        for host in [
            "192.168.1.2",
            "office-pc.local",
            "office-pc.local.",
            "::1",
            "[2001:db8::1]",
            "fe80::1%en0",
            "[fe80::1%3]",
        ] {
            assert!(ConnectionAddress::parse(host, "3389").is_ok(), "{host}");
        }
        assert_eq!(
            ConnectionAddress::parse("[::1]", "3389")
                .unwrap()
                .to_string(),
            "[::1]:3389"
        );
    }

    #[test]
    fn scoped_ipv6_survives_manual_entry_and_discovery() {
        let addr: SocketAddr = "[fe80::1%3]:5900".parse().unwrap();
        let manual = ConnectionAddress::parse("[fe80::1%3]", "5900").unwrap();
        assert_eq!(manual, ConnectionAddress::from_socket(addr));
        assert_eq!(manual.to_string(), addr.to_string());
        for host in ["fe80::1%", "fe80::1%en0/path", "fe80::1%en 0", "pc%en0"] {
            assert_eq!(
                ConnectionAddress::parse(host, "5900"),
                Err(AddressError::Host)
            );
        }
    }

    #[test]
    fn reject_urls_credentials_and_invalid_ports() {
        for host in [
            "",
            "rdp://pc",
            "user@pc",
            "pc/path",
            "pc:3389",
            "a b",
            "pc?x",
            "-pc",
            "pc..local",
        ] {
            assert_eq!(
                ConnectionAddress::parse(host, "3389"),
                Err(AddressError::Host),
                "{host}"
            );
        }
        for port in ["", "0", "65536", "-1", "rdp"] {
            assert_eq!(
                ConnectionAddress::parse("pc", port),
                Err(AddressError::Port)
            );
        }
    }
}

/// Actual milestones emitted by the protocol implementation, never a timed estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStage {
    Resolving,
    Connecting,
    Negotiating,
    Pairing,
    Authenticating,
    PreparingDesktop,
}

pub type ConnectionProgress = std::sync::Arc<dyn Fn(ConnectionStage) + Send + Sync>;

pub(crate) fn report_progress(progress: Option<&ConnectionProgress>, stage: ConnectionStage) {
    if let Some(progress) = progress {
        progress(stage);
    }
}

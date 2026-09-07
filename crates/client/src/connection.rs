//! Explicit protocol selection and address validation shared with connection forms.

use std::net::{IpAddr, Ipv6Addr, SocketAddr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

/// Credentials are per connection and deliberately excluded from Debug and persistence.
pub struct ConnectionRequest {
    pub protocol: ConnectionProtocol,
    pub address: ConnectionAddress,
    pub username: String,
    pub password: String,
    pub domain: String,
    pub accept_invalid_certificate: bool,
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

//! Public Removent addresses. Carrier URLs are an internal implementation detail.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RelayTransport {
    Quic,
    #[default]
    WebSocket,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoventEndpoint {
    pub host: String,
    pub port: u16,
}
impl RemoventEndpoint {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        let value = value.trim();
        if value.len() > 320 || value.chars().any(|c| c.is_whitespace() || c == '\\') {
            return Err("Invalid Removent address");
        }
        let url = url::Url::parse(value).map_err(|_| "Invalid Removent address")?;
        if url.scheme() != "removent"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path(), "" | "/")
        {
            return Err("Expected removent://host:port without credentials, path or query");
        }
        let port = url
            .port()
            .filter(|p| *p != 0)
            .ok_or("Removent address requires a port")?;
        let host = url::Host::parse(url.host_str().ok_or("Missing Removent host")?)
            .map_err(|_| "Invalid Removent host")?;
        let host = match host {
            url::Host::Domain(name) if !name.is_empty() => name,
            url::Host::Ipv4(ip) => ip.to_string(),
            url::Host::Ipv6(ip) => ip.to_string(),
            _ => return Err("Invalid Removent host"),
        };
        Ok(Self { host, port })
    }
    pub fn authority(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
    pub fn uri(&self) -> String {
        format!("removent://{}", self.authority())
    }
    pub fn websocket_url(&self) -> String {
        format!("wss://{}/v1/tunnel", self.authority())
    }
    pub fn is_loopback(&self) -> bool {
        matches!(self.host.as_str(), "localhost" | "127.0.0.1" | "::1")
    }
}

pub fn valid_room(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_public_scheme_with_explicit_ports_and_internal_carriers() {
        let relay = RemoventEndpoint::parse("removent://RELAY.example:443").unwrap();
        assert_eq!(relay.uri(), "removent://relay.example:443");
        assert_eq!(relay.websocket_url(), "wss://relay.example:443/v1/tunnel");
        assert_eq!(
            RemoventEndpoint::parse("removent://[::1]:48700")
                .unwrap()
                .authority(),
            "[::1]:48700"
        );
        for invalid in [
            "relay://office",
            "quic://example.com:443",
            "wss://example.com:443",
            "ws://127.0.0.1:80",
            "example.com:443",
            "removent://example.com",
            "removent://example.com:0",
            "removent://user@example.com:443",
            "removent://example.com:443/path",
            "removent://example.com:443?token=x",
            "removent://example.com:443#x",
        ] {
            assert!(RemoventEndpoint::parse(invalid).is_err(), "{invalid}");
        }
    }
}

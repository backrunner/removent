//! Network-layer errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum NetError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("quinn: {0}")]
    Quinn(#[from] quinn::ConnectionError),
    #[error("tls config: {0}")]
    Tls(String),
    #[error("endpoint: {0}")]
    Endpoint(String),
    #[error("protocol version mismatch: local={local}, remote={remote}")]
    VersionMismatch { local: u16, remote: u16 },
    #[error("handshake failed: {0}")]
    Handshake(String),
    #[error("pairing failed: {0}")]
    Pairing(String),
    #[error("peer rejected: {0}")]
    Rejected(String),
    #[error("framing: {0}")]
    Framing(String),
    #[error("discovery: {0}")]
    Discovery(String),
    #[error("timeout")]
    Timeout,
}

pub type Result<T> = std::result::Result<T, NetError>;

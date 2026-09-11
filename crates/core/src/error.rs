//! Unified error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization: {0}")]
    Serialization(String),
    #[error("certificate generation: {0}")]
    Cert(String),
    #[error("invalid data dir: {0}")]
    DataDir(String),
    #[error("another removent instance holds the data-dir lock")]
    AlreadyRunning,
    #[error("crypto: {0}")]
    Crypto(String),
    #[error("secure store: {0}")]
    SecureStore(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;

impl From<toml::ser::Error> for CoreError {
    fn from(e: toml::ser::Error) -> Self {
        CoreError::Serialization(e.to_string())
    }
}

impl From<toml::de::Error> for CoreError {
    fn from(e: toml::de::Error) -> Self {
        CoreError::Serialization(e.to_string())
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(e: serde_json::Error) -> Self {
        CoreError::Serialization(e.to_string())
    }
}

impl From<rcgen::Error> for CoreError {
    fn from(e: rcgen::Error) -> Self {
        CoreError::Cert(e.to_string())
    }
}

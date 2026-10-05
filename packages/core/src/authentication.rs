//! Native host authentication settings and authenticator setup (RFC 6238).
pub use removent_proto::AuthenticationMode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use totp_rs::{Algorithm, Secret, TOTP};

#[derive(Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthenticationSettings {
    pub mode: AuthenticationMode,
    pub pairing_policy: PairingPolicy,
    pub password: String,
    pub otp_secret: String,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingPolicy {
    #[default]
    RememberDevice,
    EveryConnection,
}
// Never expose host credentials through Settings' Debug output.
impl std::fmt::Debug for AuthenticationSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthenticationSettings")
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}
impl AuthenticationSettings {
    pub fn validate(&self) -> std::result::Result<(), String> {
        match self.mode {
            AuthenticationMode::Password if !self.mode.valid_input(&self.password) => {
                Err("Set a non-empty password (at most 1024 bytes)".into())
            }
            AuthenticationMode::Otp => self.totp().map(|_| ()),
            _ => Ok(()),
        }
    }
    pub fn generate_otp_secret() -> String {
        Secret::generate_secret().to_encoded().to_string()
    }
    pub fn totp(&self) -> std::result::Result<TOTP, String> {
        let secret = Secret::Encoded(self.otp_secret.clone())
            .to_bytes()
            .map_err(|_| "Invalid OTP secret")?;
        TOTP::new(
            Algorithm::SHA1,
            6,
            1,
            30,
            secret,
            Some("Removent".into()),
            "Remote access".into(),
        )
        .map_err(|_| "Invalid OTP secret".into())
    }
    /// Bind resumption to the current credentials; changing them invalidates old tokens.
    pub fn policy_fingerprint(&self) -> [u8; 32] {
        Sha256::digest(serde_json::to_vec(self).expect("authentication serialization")).into()
    }
    /// Persist a high-water mark, so a successful OTP cannot be reused after restart.
    pub fn consume_otp(
        &self,
        paths: &crate::DataPaths,
        step: u64,
    ) -> std::result::Result<(), String> {
        paths
            .ensure_layout()
            .map_err(|_| "Cannot save OTP replay state")?;
        let _lock = crate::DataDirLock::acquire_blocking(&paths.root.join(".otp.lock"))
            .map_err(|_| "Cannot lock OTP replay state")?;
        let file = paths.root.join("otp-used.json");
        let key = hex::encode(Sha256::digest(self.otp_secret.as_bytes()));
        let previous: Option<(String, u64)> = match std::fs::read(&file) {
            Ok(bytes) => {
                Some(serde_json::from_slice(&bytes).map_err(|_| "Invalid OTP replay state")?)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err("Cannot read OTP replay state".into()),
        };
        if previous.is_some_and(|(old, used)| old == key && used >= step) {
            return Err("OTP was already used; wait for the next code".into());
        }
        crate::settings::atomic_write(&file, &serde_json::to_vec(&(key, step)).unwrap())
            .map_err(|_| "Cannot save OTP replay state".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn totp_matches_rfc6238_vector_and_replay_state_survives_reload() {
        let auth = AuthenticationSettings {
            mode: AuthenticationMode::Otp,
            otp_secret: Secret::Raw(b"12345678901234567890".to_vec())
                .to_encoded()
                .to_string(),
            ..Default::default()
        };
        assert_eq!(auth.totp().unwrap().generate(59), "287082");
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::DataPaths {
            root: dir.path().into(),
        };
        auth.consume_otp(&paths, 1).unwrap();
        assert!(auth.clone().consume_otp(&paths, 1).is_err());
        assert!(auth.consume_otp(&paths, 0).is_err());
        auth.consume_otp(&paths, 2).unwrap();
    }
    #[test]
    fn legacy_settings_default_to_pairing_and_invalid_password_never_commits() {
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::DataPaths {
            root: dir.path().into(),
        };
        let mut settings: crate::Settings = toml::from_str("device_name = \"Legacy\"").unwrap();
        assert_eq!(
            settings.authentication.mode,
            AuthenticationMode::PairingCode
        );
        settings.save(&paths).unwrap();
        settings.authentication.mode = AuthenticationMode::Password;
        assert!(settings.save(&paths).is_err());
        assert_eq!(
            crate::Settings::load(&paths).unwrap().authentication.mode,
            AuthenticationMode::PairingCode
        );
        settings.authentication.password = "test secret".into();
        settings.save(&paths).unwrap();
        assert_eq!(
            crate::Settings::load(&paths).unwrap().authentication,
            settings.authentication
        );
        assert!(!format!("{settings:?}").contains("test secret"));
    }
}

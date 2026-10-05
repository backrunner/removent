//! Private, single-use invitations. Only the public locator may be advertised.
use crate::DataPaths;
use rand::Rng;
use serde::{Deserialize, Serialize};

pub const TTL_SECS: u64 = 300;
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone, PartialEq, Eq)]
pub struct PairingCode(String);
impl std::fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairingCode([redacted])")
    }
}
impl PairingCode {
    pub fn parse(input: &str) -> Option<Self> {
        let digits: String = input
            .chars()
            .filter(|c| *c != '-' && !c.is_ascii_whitespace())
            .collect();
        (digits.len() == 12 && digits.bytes().all(|b| b.is_ascii_digit())).then_some(Self(digits))
    }
    pub fn locator(&self) -> &str {
        &self.0[..6]
    }
    pub fn secret(&self) -> &str {
        &self.0[6..]
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn room(&self) -> String {
        format!("pair-{}", self.locator())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Advertisement {
    pub locator: String,
    pub expires_at_unix: u64,
}
impl Advertisement {
    pub fn valid(&self) -> bool {
        self.locator.len() == 6
            && self.locator.bytes().all(|b| b.is_ascii_digit())
            && self.expires_at_unix > now()
            && self.expires_at_unix <= now() + TTL_SECS
    }
    pub fn matches_room(&self, room: &str) -> bool {
        self.valid() && room == format!("pair-{}", self.locator)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Invitation {
    code: String,
    pub expires_at_unix: u64,
}
impl Invitation {
    pub fn code(&self) -> PairingCode {
        PairingCode::parse(&self.code).expect("validated invitation")
    }
    pub fn advertisement(&self) -> Advertisement {
        Advertisement {
            locator: self.code().locator().into(),
            expires_at_unix: self.expires_at_unix,
        }
    }
    pub fn generate(paths: &DataPaths) -> anyhow::Result<Self> {
        paths.ensure_layout()?;
        let _lock =
            crate::DataDirLock::acquire_blocking(&paths.root.join(".pairing-invitation.lock"))?;
        let invitation = Self {
            code: format!(
                "{:06}{:06}",
                rand::thread_rng().gen_range(0..1_000_000),
                rand::thread_rng().gen_range(0..1_000_000)
            ),
            expires_at_unix: now() + TTL_SECS,
        };
        crate::settings::atomic_write(
            &paths.root.join("pairing-invitation.json"),
            &serde_json::to_vec(&invitation)?,
        )?;
        Ok(invitation)
    }
    pub fn load(paths: &DataPaths) -> anyhow::Result<Option<Self>> {
        let bytes = match std::fs::read(paths.root.join("pairing-invitation.json")) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let value: Self = serde_json::from_slice(&bytes)?;
        anyhow::ensure!(
            PairingCode::parse(&value.code).is_some(),
            "Invalid invitation state"
        );
        Ok((value.expires_at_unix > now()).then_some(value))
    }
    pub fn revoke(paths: &DataPaths) -> anyhow::Result<()> {
        let _lock =
            crate::DataDirLock::acquire_blocking(&paths.root.join(".pairing-invitation.lock"))?;
        Self::remove(paths)
    }
    fn remove(paths: &DataPaths) -> anyhow::Result<()> {
        match std::fs::remove_file(paths.root.join("pairing-invitation.json")) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn consume(&self, paths: &DataPaths) -> anyhow::Result<()> {
        let _lock =
            crate::DataDirLock::acquire_blocking(&paths.root.join(".pairing-invitation.lock"))?;
        anyhow::ensure!(
            Self::load(paths)?
                .is_some_and(|active| active.code == self.code
                    && active.expires_at_unix == self.expires_at_unix),
            "Invitation expired or already used"
        );
        Self::remove(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invitation_is_private_single_use_and_replacing_invalidates_previous() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths {
            root: dir.path().into(),
        };
        let old = Invitation::generate(&paths).unwrap();
        let new = Invitation::generate(&paths).unwrap();
        assert!(old.consume(&paths).is_err());
        let code = new.code();
        assert_eq!(
            PairingCode::parse(&format!("{}-{}", code.locator(), code.secret())),
            Some(code.clone())
        );
        assert!(!format!("{code:?}").contains(code.secret()));
        let public = serde_json::to_value(new.advertisement()).unwrap();
        assert_eq!(public["locator"], code.locator());
        assert!(public.get("code").is_none());
        assert!(public.get("secret").is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(paths.root.join("pairing-invitation.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        new.consume(&paths).unwrap();
        assert!(new.consume(&paths).is_err());
        assert!(Invitation::load(&paths).unwrap().is_none());
    }
}

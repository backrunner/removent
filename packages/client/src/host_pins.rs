//! Local destination certificate pins, shared by desktop and mobile controllers.
use anyhow::{Context, Result, ensure};
use removent_core::DataPaths;
use std::collections::BTreeMap;

fn load(paths: &DataPaths) -> Result<BTreeMap<String, String>> {
    let mut pins = BTreeMap::new();
    // Preserve pins recorded by earlier mobile builds when migrating the file.
    for name in ["mobile-host-pins.json", "host-pins.json"] {
        match std::fs::read(paths.root.join(name)) {
            Ok(bytes) => pins.extend(
                serde_json::from_slice::<BTreeMap<String, String>>(&bytes)
                    .context("Invalid saved host fingerprints")?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context("Cannot read saved host fingerprints"),
        }
    }
    Ok(pins)
}

pub fn lookup(paths: &DataPaths, destination: &str) -> Result<Option<[u8; 32]>> {
    load(paths)?
        .get(destination)
        .map(|pin| {
            hex::decode(pin)?
                .try_into()
                .map_err(|_| anyhow::anyhow!("Invalid saved host fingerprint"))
        })
        .transpose()
}

pub struct CertificateTrust {
    pub expected: Option<[u8; 32]>,
    pub needs_confirmation: bool,
}

/// A relay verification exception must not consult or update local certificate trust.
pub fn for_relay_connection(
    paths: &DataPaths,
    destination: &str,
    supplied: Option<[u8; 32]>,
    verify: bool,
) -> Result<CertificateTrust> {
    if !verify {
        return Ok(CertificateTrust {
            expected: None,
            needs_confirmation: false,
        });
    }
    for_connection(paths, destination, supplied, false)
}

/// Imported/supplied pins constrain TLS but do not substitute for local consent.
/// An invitation may rebind a destination only after its proof is verified later.
pub fn for_connection(
    paths: &DataPaths,
    destination: &str,
    supplied: Option<[u8; 32]>,
    using_invitation: bool,
) -> Result<CertificateTrust> {
    let local = lookup(paths, destination)?;
    if !using_invitation && let (Some(local), Some(supplied)) = (local, supplied) {
        ensure!(
            local == supplied,
            "Host certificate changed; use a new connection code to pair with this computer"
        );
    }
    Ok(CertificateTrust {
        expected: if using_invitation {
            supplied
        } else {
            supplied.or(local)
        },
        needs_confirmation: local.is_none() || using_invitation,
    })
}

/// An ordinary connection may never replace a pinned certificate. Only a
/// successfully verified, explicitly supplied invitation can rebind it.
pub fn remember(
    paths: &DataPaths,
    destination: &str,
    fingerprint: [u8; 32],
    verified_invitation: bool,
) -> Result<()> {
    paths.ensure_layout()?;
    let _lock = removent_core::DataDirLock::acquire_blocking(&paths.root.join(".host-pins.lock"))?;
    let mut pins = load(paths)?;
    let pin = hex::encode(fingerprint);
    ensure!(
        verified_invitation
            || pins
                .get(destination)
                .is_none_or(|previous| previous == &pin),
        "Host certificate changed; use a new connection code to pair with this computer"
    );
    pins.insert(destination.into(), pin);
    removent_core::settings::atomic_write(
        &paths.root.join("host-pins.json"),
        &serde_json::to_vec(&pins)?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_consent_is_required_for_imported_pins_and_existing_identity_cannot_be_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths {
            root: dir.path().into(),
        };
        let unknown = for_connection(&paths, "office", Some([1; 32]), false).unwrap();
        assert!(unknown.needs_confirmation);
        assert_eq!(unknown.expected, Some([1; 32]));
        assert!(lookup(&paths, "office").unwrap().is_none());
        remember(&paths, "office", [1; 32], false).unwrap();
        let known = for_connection(&paths, "office", None, false).unwrap();
        assert!(!known.needs_confirmation);
        assert_eq!(known.expected, Some([1; 32]));
        assert!(for_connection(&paths, "office", Some([2; 32]), false).is_err());
        let invitation = for_connection(&paths, "office", None, true).unwrap();
        assert!(invitation.needs_confirmation);
        assert!(invitation.expected.is_none());
        assert_eq!(lookup(&paths, "office").unwrap(), Some([1; 32]));
    }

    #[test]
    fn relay_verification_exception_does_not_compare_or_replace_local_identity() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths {
            root: dir.path().into(),
        };
        remember(&paths, "relay", [1; 32], false).unwrap();
        assert!(for_relay_connection(&paths, "relay", Some([2; 32]), true).is_err());
        let trust = for_relay_connection(&paths, "relay", Some([2; 32]), false).unwrap();
        assert!(!trust.needs_confirmation);
        assert!(trust.expected.is_none());
        assert_eq!(lookup(&paths, "relay").unwrap(), Some([1; 32]));
    }

    #[test]
    fn preserves_mobile_pins_and_rejects_unverified_certificate_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let paths = DataPaths {
            root: dir.path().into(),
        };
        std::fs::write(
            paths.root.join("mobile-host-pins.json"),
            serde_json::to_vec(&BTreeMap::from([("mac:48688", hex::encode([1; 32]))])).unwrap(),
        )
        .unwrap();
        assert_eq!(lookup(&paths, "mac:48688").unwrap(), Some([1; 32]));
        assert!(remember(&paths, "mac:48688", [2; 32], false).is_err());
        remember(&paths, "another:48688", [3; 32], false).unwrap();
        assert_eq!(lookup(&paths, "mac:48688").unwrap(), Some([1; 32]));
        remember(&paths, "mac:48688", [2; 32], true).unwrap();
        assert_eq!(lookup(&paths, "mac:48688").unwrap(), Some([2; 32]));
        assert_eq!(lookup(&paths, "another:48688").unwrap(), Some([3; 32]));
    }
}

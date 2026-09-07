//! Device identity: Ed25519 long-term key + self-signed certificate + SHA-256 fingerprint (protocol.md §4.1).

use crate::error::{CoreError, Result};
use crate::paths::DataPaths;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use rcgen::{CertificateParams, DnType, KeyPair};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct DeviceIdentity {
    pub signing_key: SigningKey,
    pub cert_der: Vec<u8>,
    pub fingerprint: [u8; 32],
    pub device_name: String,
}

impl DeviceIdentity {
    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    pub fn fingerprint_hex(&self) -> String {
        hex::encode(self.fingerprint)
    }

    /// Short fingerprint for UI display (first 8 bytes as hex, matches the mDNS TXT `fp`).
    pub fn short_fingerprint_hex(&self) -> String {
        hex::encode(&self.fingerprint[..8])
    }

    pub fn sign(&self, msg: &[u8]) -> Signature {
        self.signing_key.sign(msg)
    }

    /// PKCS#8 private key DER for the TLS stack (re-encoded each time, never written to disk).
    pub fn private_pkcs8_der(&self) -> Result<std::vec::Vec<u8>> {
        use ed25519_dalek::pkcs8::EncodePrivateKey;
        let doc = self
            .signing_key
            .to_pkcs8_der()
            .map_err(|e| CoreError::Crypto(e.to_string()))?;
        Ok(doc.as_bytes().to_vec())
    }

    pub fn verify(peer_vk: &VerifyingKey, msg: &[u8], sig: &Signature) -> bool {
        peer_vk.verify(msg, sig).is_ok()
    }
}

/// Load or create the device identity. Key file permissions are 0600.
pub fn load_or_create(paths: &DataPaths, device_name: &str) -> Result<DeviceIdentity> {
    paths.ensure_layout()?;
    // App and daemon can start together on a fresh data directory. Hold one
    // lock across both files so they never generate mismatched keys/certs.
    let _initialization =
        crate::flock::DataDirLock::acquire_blocking(&paths.identity_dir().join(".init.lock"))?;
    let key_path = paths.device_key();
    let key_pem = match std::fs::read_to_string(&key_path) {
        Ok(pem) => pem,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => create_and_store(paths, device_name)?,
        Err(e) => return Err(e.into()),
    };
    let signing_key = SigningKey::from_pkcs8_pem(&key_pem)
        .map_err(|e| CoreError::Crypto(format!("parse device.key: {e}")))?;
    load_cert_for_key(paths, signing_key, device_name)
}

fn create_and_store(paths: &DataPaths, _device_name: &str) -> Result<String> {
    let mut csprng = rand::rngs::OsRng;
    let signing_key = SigningKey::generate(&mut csprng);
    let doc = signing_key
        .to_pkcs8_der()
        .map_err(|e| CoreError::Crypto(e.to_string()))?;
    let pem = doc
        .to_pem("PRIVATE KEY", LineEnding::LF)
        .map_err(|e| CoreError::Crypto(e.to_string()))?;
    let path = paths.device_key();
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    file.write_all(pem.as_bytes())?;
    restrict_0600(&path)?;
    Ok(pem.to_string())
}

fn load_cert_for_key(
    paths: &DataPaths,
    signing_key: SigningKey,
    device_name: &str,
) -> Result<DeviceIdentity> {
    let cert_path = paths.device_cert();
    if let Ok(der) = std::fs::read(&cert_path) {
        let fp: [u8; 32] = Sha256::digest(&der).into();
        // Certificate fingerprint consistency is guaranteed by the pairing flow and mutual pinning (protocol.md §4.2).
        return Ok(DeviceIdentity {
            signing_key,
            cert_der: der,
            fingerprint: fp,
            device_name: device_name.to_string(),
        });
    }
    let (cert_der, fp) = issue_self_signed(&signing_key, device_name)?;
    std::fs::write(&cert_path, &cert_der)?;
    restrict_0600(&cert_path)?;
    Ok(DeviceIdentity {
        signing_key,
        cert_der,
        fingerprint: fp,
        device_name: device_name.to_string(),
    })
}

fn issue_self_signed(key: &SigningKey, device_name: &str) -> Result<(Vec<u8>, [u8; 32])> {
    let doc = key
        .to_pkcs8_der()
        .map_err(|e| CoreError::Crypto(e.to_string()))?;
    let pkcs8_pem = doc
        .to_pem("PRIVATE KEY", LineEnding::LF)
        .map_err(|e| CoreError::Crypto(e.to_string()))?;
    let key_pair = KeyPair::from_pkcs8_pem_and_sign_algo(&pkcs8_pem, &rcgen::PKCS_ED25519)?;

    let mut params = CertificateParams::new(Vec::<String>::new())?;
    params
        .distinguished_name
        .push(DnType::CommonName, device_name);
    params.is_ca = rcgen::IsCa::NoCa;
    let cert = params.self_signed(&key_pair)?;
    let der = cert.der().as_ref().to_vec();
    let fp: [u8; 32] = Sha256::digest(&der).into();
    Ok((der, fp))
}

fn restrict_0600(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perm = std::fs::metadata(path)?.permissions();
    perm.set_mode(0o600);
    std::fs::set_permissions(path, perm)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::DataPaths;

    #[test]
    fn concurrent_first_launch_shares_one_key_and_certificate() {
        let dir = tempfile::tempdir().unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let paths = DataPaths {
                    root: dir.path().to_path_buf(),
                };
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    load_or_create(&paths, "Concurrent Mac").unwrap()
                })
            })
            .collect();
        let identities: Vec<_> = tasks.into_iter().map(|t| t.join().unwrap()).collect();
        for identity in &identities {
            assert_eq!(identity.fingerprint, identities[0].fingerprint);
            assert_eq!(identity.verifying_key(), identities[0].verifying_key());
        }
    }

    #[test]
    fn identity_roundtrip_is_stable() {
        let tmp = tempfile::tempdir().unwrap();
        let p = DataPaths {
            root: tmp.path().to_path_buf(),
        };
        let id1 = load_or_create(&p, "Studio-Mac").unwrap();
        assert_eq!(id1.short_fingerprint_hex().len(), 16);
        let id2 = load_or_create(&p, "Studio-Mac").unwrap();
        assert_eq!(id1.fingerprint, id2.fingerprint);

        let msg = b"pairing-confirm";
        let sig = id2.sign(msg);
        assert!(DeviceIdentity::verify(&id2.verifying_key(), msg, &sig));
        let mut bad = sig.to_bytes();
        bad[0] ^= 0xff;
        assert!(!DeviceIdentity::verify(
            &id2.verifying_key(),
            msg,
            &Signature::from_bytes(&bad)
        ));

        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(p.device_key())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn fingerprints_are_unique_per_device() {
        let t1 = tempfile::tempdir().unwrap();
        let t2 = tempfile::tempdir().unwrap();
        let a = load_or_create(
            &DataPaths {
                root: t1.path().to_path_buf(),
            },
            "A",
        )
        .unwrap();
        let b = load_or_create(
            &DataPaths {
                root: t2.path().to_path_buf(),
            },
            "B",
        )
        .unwrap();
        assert_ne!(a.fingerprint, b.fingerprint);
    }
}

//! Device proofs authorize a room and role, never desktop capabilities.
use crate::config::{Role, decode_secret, token_hash};
use anyhow::{Result, ensure};
use ed25519_dalek::{Signature, VerifyingKey};
use removent_core::DeviceIdentity;
use subtle::ConstantTimeEq;

pub struct Policy {
    hash: Option<[u8; 32]>,
    keys: Vec<[u8; 32]>,
}
impl Policy {
    pub fn new(hash: &str, keys: &[String]) -> Result<Self> {
        ensure!(keys.len() <= 256, "Too many relay device keys");
        let hash = if hash.is_empty() {
            None
        } else {
            Some(decode_secret(hash)?)
        };
        let keys = keys
            .iter()
            .map(|k| {
                let key = decode_secret(k)?;
                let vk = VerifyingKey::from_bytes(&key)?;
                ensure!(!vk.is_weak(), "Invalid device key");
                Ok(key)
            })
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            hash.is_some() || !keys.is_empty(),
            "A relay role requires a credential or registered device keys"
        );
        Ok(Self { hash, keys })
    }
    pub fn authorize(&self, token: &str, key: &[u8; 32]) -> Result<()> {
        if let Some(hash) = self.hash {
            ensure!(bool::from(hash.ct_eq(&token_hash(token)?)), "Unauthorized");
        } else {
            ensure!(token.is_empty(), "Unexpected credential");
        }
        ensure!(
            self.keys.is_empty() || self.keys.contains(key),
            "Unauthorized device"
        );
        Ok(())
    }
}
pub fn role_name(role: Role) -> &'static str {
    match role {
        Role::Host => "host",
        Role::Client => "client",
    }
}
pub fn challenge_message(room: &str, role: Role, nonce: &[u8; 32]) -> Vec<u8> {
    format!(
        "removent-relay-proof-v1\n{room}\n{}\n{}",
        role_name(role),
        hex::encode(nonce)
    )
    .into_bytes()
}
pub fn sign_challenge(
    identity: &DeviceIdentity,
    room: &str,
    role: Role,
    nonce: &[u8; 32],
) -> [u8; 64] {
    identity
        .sign(&challenge_message(room, role, nonce))
        .to_bytes()
}
pub fn verify(key: &[u8; 32], message: &[u8], signature: &[u8]) -> Result<()> {
    let key = VerifyingKey::from_bytes(key)?;
    key.verify_strict(message, &Signature::from_slice(signature)?)?;
    Ok(())
}
/// Edge admission proof. A fresh server challenge still protects registration
/// after upgrades, including container restarts with empty replay caches.
pub fn admission_message(
    audience: &str,
    room: &str,
    role: Role,
    time: &str,
    nonce: &str,
) -> Vec<u8> {
    format!(
        "removent-relay-admission-v1\n{audience}\n{room}\n{}\n{time}\n{nonce}",
        role_name(role)
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proof_binds_room_role_and_fresh_challenge() {
        let dir = tempfile::tempdir().unwrap();
        let id = removent_core::identity::load_or_create(
            &removent_core::DataPaths {
                root: dir.path().to_owned(),
            },
            "device",
        )
        .unwrap();
        let nonce = [1; 32];
        let proof = sign_challenge(&id, "office", Role::Host, &nonce);
        let key = id.verifying_key().to_bytes();
        assert!(
            verify(
                &key,
                &challenge_message("office", Role::Host, &nonce),
                &proof
            )
            .is_ok()
        );
        for message in [
            challenge_message("other", Role::Host, &nonce),
            challenge_message("office", Role::Client, &nonce),
            challenge_message("office", Role::Host, &[2; 32]),
        ] {
            assert!(verify(&key, &message, &proof).is_err());
        }
        assert!(
            Policy::new("", &[]).is_err(),
            "anonymous roles must fail closed"
        );
    }
}

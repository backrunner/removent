//! SPAKE2 pairing protocol (protocol.md §4.3).
//!
//! Flow: Begin(nonce_c,fp_c) → Challenge(nonce_h,msg_h; host displays the PIN) →
//! Verify(msg_c,confirm_c,sig_c) → Confirm(ok,confirm_h,sig_h).
//! The confirm binds both full fingerprints; the signature binds the long-term identity key.

use crate::error::{NetError, Result};
use ed25519_dalek::Verifier;
use rand::RngCore;
use removent_core::DeviceIdentity;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::time::{Duration, Instant};

/// PIN validity period (protocol.md §4.3): the host always rejects expired PINs.
pub const PIN_TTL: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PairingMsg {
    Begin {
        nonce_c: [u8; 16],
        fp_c: String,
    },
    Challenge {
        nonce_h: [u8; 16],
        msg_h: Vec<u8>,
    },
    Verify {
        msg_c: Vec<u8>,
        confirm_c: [u8; 32],
        /// Client long-term public key (raw 32B), for the host to verify the signature; identity authentication is handled by TLS fingerprint pinning.
        vk_c: [u8; 32],
        sig_c: Vec<u8>,
    },
    Confirm {
        ok: bool,
        confirm_h: [u8; 32],
        sig_h: Vec<u8>,
    },
}

const CLIENT_ROLE_ID: &[u8] = b"removent-client";
const HOST_ROLE_ID: &[u8] = b"removent-host";

pub fn generate_pin() -> String {
    let mut buf = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut buf);
    format!("{:06}", u32::from_le_bytes(buf) % 1_000_000)
}

fn transcript_confirm(
    shared: &[u8],
    nonce_c: &[u8; 16],
    nonce_h: &[u8; 16],
    fp_c_hex: &str,
    fp_h_hex: &str,
) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"RVP1-pairing-confirm");
    h.update(shared);
    h.update(nonce_c);
    h.update(nonce_h);
    h.update(fp_c_hex.as_bytes());
    h.update(fp_h_hex.as_bytes());
    h.finalize().into()
}

fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

fn verify_sig(vk: &ed25519_dalek::VerifyingKey, confirm: &[u8; 32], sig: &[u8]) -> bool {
    let Ok(arr) = <[u8; 64]>::try_from(sig) else {
        return false;
    };
    vk.verify(confirm, &ed25519_dalek::Signature::from_bytes(&arr))
        .is_ok()
}

// ---------- Client side ----------

/// Step 1: generate the Begin message (nonce_c must be kept for step 2 and the final check).
pub fn client_begin(fp_self_full_hex: &str) -> (PairingMsg, [u8; 16]) {
    let mut nonce_c = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut nonce_c);
    (
        PairingMsg::Begin {
            nonce_c,
            fp_c: fp_self_full_hex.to_string(),
        },
        nonce_c,
    )
}

/// Step 2: after receiving the Challenge and the user-entered PIN, produce the Verify message and the shared key.
/// `fp_peer_full_hex` is the peer certificate fingerprint recorded during the TLS handshake.
pub fn client_verify(
    nonce_c: &[u8; 16],
    challenge: &PairingMsg,
    fp_self_full_hex: &str,
    fp_peer_full_hex: &str,
    pin: &str,
    id: &DeviceIdentity,
) -> Result<(PairingMsg, Vec<u8>)> {
    let PairingMsg::Challenge { nonce_h, msg_h } = challenge else {
        return Err(NetError::Pairing("expected Challenge".into()));
    };
    let (spake, msg_c) = Spake2::<Ed25519Group>::start_a(
        &Password::new(pin.as_bytes()),
        &Identity::new(CLIENT_ROLE_ID),
        &Identity::new(HOST_ROLE_ID),
    );
    let shared = spake
        .finish(msg_h)
        .map_err(|e| NetError::Pairing(format!("spake2: {e}")))?
        .to_vec();
    let confirm = transcript_confirm(
        &shared,
        nonce_c,
        nonce_h,
        fp_self_full_hex,
        fp_peer_full_hex,
    );
    Ok((
        PairingMsg::Verify {
            msg_c,
            confirm_c: confirm,
            vk_c: *id.verifying_key().as_bytes(),
            sig_c: id.sign(&confirm).to_bytes().to_vec(),
        },
        shared,
    ))
}

/// Client-side check of the server's Confirm; returns a copy of the shared key on success.
#[allow(clippy::too_many_arguments)]
pub fn client_confirm_check(
    shared: &[u8],
    nonce_c: &[u8; 16],
    nonce_h: &[u8; 16],
    fp_self_full_hex: &str,
    fp_peer_full_hex: &str,
    confirm: &PairingMsg,
    peer_vk: &ed25519_dalek::VerifyingKey,
) -> Result<()> {
    let PairingMsg::Confirm {
        ok,
        confirm_h,
        sig_h,
    } = confirm
    else {
        return Err(NetError::Pairing("expected Confirm".into()));
    };
    if !ok {
        return Err(NetError::Rejected(
            "PIN mismatch or pairing rejected".into(),
        ));
    }
    let expect = transcript_confirm(shared, nonce_c, nonce_h, fp_self_full_hex, fp_peer_full_hex);
    if !constant_time_eq(&expect, confirm_h) {
        return Err(NetError::Pairing("confirm MAC mismatch".into()));
    }
    if !verify_sig(peer_vk, confirm_h, sig_h) {
        return Err(NetError::Pairing("host signature invalid".into()));
    }
    Ok(())
}

// ---------- Server side ----------

pub struct HostHandshake {
    pub pin: String,
    pub reply: PairingMsg,
    pub nonce_c: [u8; 16],
    nonce_h: [u8; 16],
    spake: Option<Spake2<Ed25519Group>>,
    /// PIN generation time, used for [`PIN_TTL`] expiry checks.
    created_at: Instant,
}

/// On Begin received: generate the PIN and prepare the Challenge reply.
pub fn host_on_begin(begin: &PairingMsg) -> Result<HostHandshake> {
    let PairingMsg::Begin { nonce_c, .. } = begin else {
        return Err(NetError::Pairing("expected Begin".into()));
    };
    let mut nonce_h = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut nonce_h);
    let pin = generate_pin();
    let (spake, msg_h) = Spake2::<Ed25519Group>::start_b(
        &Password::new(pin.as_bytes()),
        &Identity::new(CLIENT_ROLE_ID),
        &Identity::new(HOST_ROLE_ID),
    );
    Ok(HostHandshake {
        pin,
        reply: PairingMsg::Challenge { nonce_h, msg_h },
        nonce_c: *nonce_c,
        nonce_h,
        spake: Some(spake),
        created_at: Instant::now(),
    })
}

/// On Verify received: check the PIN-derived key and client signature, reply with Confirm.
/// On success returns `(Confirm(ok=true), Some(shared))`; wrong or expired PIN returns `(Confirm(ok=false), None)`.
pub fn host_verify(
    mut hs: HostHandshake,
    verify: &PairingMsg,
    host_id: &DeviceIdentity,
    fp_client_full_hex: &str,
) -> Result<(PairingMsg, Option<Vec<u8>>)> {
    // PIN expired: reject outright without consuming the SPAKE state.
    if hs.created_at.elapsed() > PIN_TTL {
        return Ok((
            PairingMsg::Confirm {
                ok: false,
                confirm_h: [0u8; 32],
                sig_h: vec![],
            },
            None,
        ));
    }
    let PairingMsg::Verify {
        msg_c,
        confirm_c,
        vk_c,
        sig_c,
    } = verify
    else {
        return Err(NetError::Pairing("expected Verify".into()));
    };
    let Ok(vk_arr) = <[u8; 32]>::try_from(vk_c.as_slice()) else {
        return Err(NetError::Pairing("bad vk length".into()));
    };
    let client_vk = ed25519_dalek::VerifyingKey::from_bytes(&vk_arr)
        .map_err(|e| NetError::Pairing(format!("vk: {e}")))?;
    let spake = hs
        .spake
        .take()
        .ok_or_else(|| NetError::Pairing("state consumed".into()))?;
    let shared = spake
        .finish(msg_c)
        .map_err(|e| NetError::Pairing(format!("spake2: {e}")))?
        .to_vec();
    let fp_h = host_id.fingerprint_hex();
    let expect = transcript_confirm(&shared, &hs.nonce_c, &hs.nonce_h, fp_client_full_hex, &fp_h);
    let ok = constant_time_eq(&expect, confirm_c) && verify_sig(&client_vk, confirm_c, sig_c);
    let reply = PairingMsg::Confirm {
        ok,
        confirm_h: expect,
        sig_h: if ok {
            host_id.sign(&expect).to_bytes().to_vec()
        } else {
            vec![]
        },
    };
    Ok((reply, if ok { Some(shared) } else { None }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use removent_core::{DataPaths, identity};

    fn fake_identity(name: &str) -> (DeviceIdentity, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let p = DataPaths {
            root: dir.path().to_path_buf(),
        };
        (identity::load_or_create(&p, name).unwrap(), dir)
    }

    #[test]
    fn pin_is_six_digits() {
        assert!(generate_pin().parse::<u32>().is_ok());
        assert_eq!(generate_pin().len(), 6);
    }

    #[test]
    fn pairing_flow_success_with_matching_pin() {
        let (client_id, _c) = fake_identity("ClientMac");
        let (host_id, _h) = fake_identity("HostMac");
        let fp_c = client_id.fingerprint_hex();
        let fp_h = host_id.fingerprint_hex();

        let (begin, nonce_c) = client_begin(&fp_c);
        let hs = host_on_begin(&begin).unwrap();
        let pin = hs.pin.clone();
        let PairingMsg::Challenge { nonce_h, .. } = &hs.reply else {
            panic!("expected Challenge");
        };
        let nonce_h = *nonce_h;

        let (verify, shared_c) =
            client_verify(&nonce_c, &hs.reply, &fp_c, &fp_h, &pin, &client_id).unwrap();

        let (confirm, shared_opt) = host_verify(hs, &verify, &host_id, &fp_c).unwrap();
        assert!(matches!(confirm, PairingMsg::Confirm { ok: true, .. }));
        let shared_h = shared_opt.unwrap();
        assert_eq!(shared_c, shared_h);

        client_confirm_check(
            &shared_c,
            &nonce_c,
            &nonce_h,
            &fp_c,
            &fp_h,
            &confirm,
            &host_id.verifying_key(),
        )
        .unwrap();
    }

    #[test]
    fn wrong_pin_rejected_by_mac_mismatch() {
        let (client_id, _c) = fake_identity("C2");
        let (host_id, _h) = fake_identity("H2");
        let fp_c = client_id.fingerprint_hex();
        let fp_h = host_id.fingerprint_hex();

        let (begin, nonce_c) = client_begin(&fp_c);
        let hs = host_on_begin(&begin).unwrap();
        let real_pin = hs.pin.clone();
        let wrong = if real_pin == "000000" {
            "000001"
        } else {
            "000000"
        };

        let (verify, _) =
            client_verify(&nonce_c, &hs.reply, &fp_c, &fp_h, wrong, &client_id).unwrap();
        let (confirm, shared) = host_verify(hs, &verify, &host_id, &fp_c).unwrap();
        assert!(matches!(confirm, PairingMsg::Confirm { ok: false, .. }));
        assert!(shared.is_none());
        let _ = real_pin;
    }

    #[test]
    fn tampered_signature_rejected() {
        let (client_id, _c) = fake_identity("C3");
        let (host_id, _h) = fake_identity("H3");
        let fp_c = client_id.fingerprint_hex();
        let fp_h = host_id.fingerprint_hex();

        let (begin, nonce_c) = client_begin(&fp_c);
        let hs = host_on_begin(&begin).unwrap();
        let pin = hs.pin.clone();
        let (mut verify, _) =
            client_verify(&nonce_c, &hs.reply, &fp_c, &fp_h, &pin, &client_id).unwrap();
        let PairingMsg::Verify { sig_c, .. } = &mut verify else {
            panic!()
        };
        if let Some(b) = sig_c.first_mut() {
            *b ^= 0xff;
        }
        let (confirm, shared) = host_verify(hs, &verify, &host_id, &fp_c).unwrap();
        assert!(matches!(confirm, PairingMsg::Confirm { ok: false, .. }));
        assert!(shared.is_none());
    }

    #[test]
    fn expired_pin_rejected() {
        let (client_id, _c) = fake_identity("C4");
        let (host_id, _h) = fake_identity("H4");
        let fp_c = client_id.fingerprint_hex();
        let fp_h = host_id.fingerprint_hex();

        let (begin, nonce_c) = client_begin(&fp_c);
        let mut hs = host_on_begin(&begin).unwrap();
        let pin = hs.pin.clone();
        // Artificially move the PIN generation time back before the TTL.
        hs.created_at = Instant::now() - PIN_TTL - Duration::from_secs(1);

        let (verify, _) =
            client_verify(&nonce_c, &hs.reply, &fp_c, &fp_h, &pin, &client_id).unwrap();
        let (confirm, shared) = host_verify(hs, &verify, &host_id, &fp_c).unwrap();
        assert!(matches!(confirm, PairingMsg::Confirm { ok: false, .. }));
        assert!(shared.is_none());
    }
}

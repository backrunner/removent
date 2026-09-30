use super::*;

/// Release-signing public key (Ed25519, hex). The private key only lives in the
/// CI secrets of the release pipeline (release.md §2).
const RELEASE_PUBLIC_KEY_HEX: &str =
    "2b51160721f9eee916944e4603e6540c628247e5284a79b099392f086a3def0c";

pub fn parse_version(s: &str) -> Option<semver::Version> {
    semver::Version::parse(s.trim().strip_prefix('v').unwrap_or(s.trim())).ok()
}

pub fn is_newer(remote: &str, local: &str) -> bool {
    match (parse_version(remote), parse_version(local)) {
        (Some(r), Some(l)) => r.cmp_precedence(&l).is_gt(),
        _ => false,
    }
}

pub(super) fn channel_accepts(channel: UpdateChannel, version: &semver::Version) -> bool {
    version.pre.is_empty()
        || (channel == UpdateChannel::Beta
            && version
                .pre
                .as_str()
                .strip_prefix("beta.")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())))
}

pub(super) fn eligible_upgrade(channel: UpdateChannel, remote: &str, local: &str) -> bool {
    parse_version(remote).is_some_and(|v| channel_accepts(channel, &v)) && is_newer(remote, local)
}

/// Ed25519 signature payload: `"{version}\n{url}\n{sha256}\n{min_compatible_proto}"`
/// (four lines, no trailing newline — contract with the release pipeline).
pub fn signature_payload(m: &UpdateManifest) -> Vec<u8> {
    format!(
        "{}\n{}\n{}\n{}",
        m.version, m.url, m.sha256, m.min_compatible_proto
    )
    .into_bytes()
}

/// Verify the manifest's detached Ed25519 signature (64-byte hex) against the
/// release public key.
pub fn verify_manifest_signature(m: &UpdateManifest, key: &VerifyingKey) -> bool {
    let sig_bytes = match hex::decode(m.signature.trim()) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let Ok(sig_arr) = <[u8; 64]>::try_from(sig_bytes.as_slice()) else {
        return false;
    };
    let sig = ed25519_dalek::Signature::from_bytes(&sig_arr);
    key.verify_strict(&signature_payload(m), &sig).is_ok()
}

/// The compiled-in release verification key (None only on a corrupted constant).
pub fn release_verifying_key() -> Option<VerifyingKey> {
    let bytes = hex::decode(RELEASE_PUBLIC_KEY_HEX).ok()?;
    let arr = <[u8; 32]>::try_from(bytes.as_slice()).ok()?;
    VerifyingKey::from_bytes(&arr).ok()
}

// ---- check ----

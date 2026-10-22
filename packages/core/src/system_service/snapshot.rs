use super::*;
use crate::peers::PeerRecord;
use std::collections::BTreeSet;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

pub(super) fn trusted_peers(paths: &DataPaths) -> Result<Vec<PeerRecord>> {
    let bytes = match security::read_regular(&paths.peers_file()) {
        Ok(bytes) => bytes,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            b"[]".to_vec()
        }
        Err(error) => return Err(error),
    };
    let mut peers: Vec<PeerRecord> = serde_json::from_slice(&bytes)?;
    peers.retain(|p| p.trusted && p.granted_caps.video);
    let mut fingerprints = BTreeSet::new();
    for peer in &mut peers {
        ensure!(
            hex::decode(&peer.fingerprint).is_ok_and(|fp| fp.len() == 32),
            "Invalid approved device fingerprint"
        );
        ensure!(
            fingerprints.insert(peer.fingerprint.to_lowercase()),
            "Duplicate approved device"
        );
        peer.fingerprint.make_ascii_lowercase();
        peer.short_fp = peer.fingerprint[..16].to_owned();
        peer.granted_caps.audio = false;
        peer.granted_caps.clipboard = false;
        peer.granted_caps.file = false;
    }
    peers.sort_by(|a, b| a.fingerprint.cmp(&b.fingerprint));
    Ok(peers)
}

pub(super) fn approved_devices(peers: &[PeerRecord]) -> Vec<ApprovedDevice> {
    peers
        .iter()
        .map(|p| ApprovedDevice {
            fingerprint: p.fingerprint.clone(),
            name: p.name.clone(),
            input: p.granted_caps.input,
        })
        .collect()
}

pub(super) fn write_file(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    use std::fs;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub(super) fn stage(paths: &DataPaths, staged: &Path) -> Result<SystemServiceStatus> {
    use sha2::{Digest, Sha256};
    let key = security::read_regular(&paths.device_key())?;
    let cert = security::read_regular(&paths.device_cert())?;
    validate_identity(&key, &cert)?;
    let peers = trusted_peers(paths)?;
    let settings_bytes = match security::read_regular(&paths.settings_file()) {
        Ok(bytes) => bytes,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            Vec::new()
        }
        Err(error) => return Err(error),
    };
    let original: Settings =
        toml::from_str(std::str::from_utf8(&settings_bytes)?).map_err(|_| {
            anyhow::anyhow!("Invalid host settings; repair settings before installation")
        })?;
    let settings = Settings {
        device_name: original.device_name,
        host_port: original.host_port,
        host_enabled: true,
        paired_only: true,
        window_server_capture: true,
        vnc_enabled: false,
        audio_enabled_default: false,
        update_check_enabled: false,
        prevent_idle_sleep: original.prevent_idle_sleep,
        admission: crate::AdmissionMode::TrustedAuto,
        // Never copy an app-managed password/OTP into a second authority. The
        // pre-login host authenticates macOS accounts and preapproved devices.
        authentication: crate::authentication::AuthenticationSettings {
            mode: crate::AuthenticationMode::SystemAccount,
            ..Default::default()
        },
        ..Settings::default()
    };
    let data = staged.join("data");
    for (name, bytes) in [
        ("identity/device.key", key),
        ("identity/device.crt", cert.clone()),
        ("peers.json", serde_json::to_vec_pretty(&peers)?),
        (
            "settings.toml",
            toml::to_string_pretty(&settings)?.into_bytes(),
        ),
        ("run/service-stopped", b"Installing\n".to_vec()),
    ] {
        write_file(&data.join(name), &bytes, 0o600)?;
    }
    let relay = paths.root.join("relay-host.toml");
    if relay.try_exists()? {
        let bytes = security::read_regular(&relay)?;
        let _: toml::Table = toml::from_str(std::str::from_utf8(&bytes)?)
            .map_err(|_| anyhow::anyhow!("Invalid relay host settings"))?;
        write_file(&data.join("relay-host.toml"), &bytes, 0o600)?;
    }
    let status = SystemServiceStatus {
        installed: true,
        enabled: true,
        version: env!("CARGO_PKG_VERSION").into(),
        host_fingerprint: hex::encode(Sha256::digest(cert)),
        approved_devices: approved_devices(&peers),
    };
    write_file(
        &staged.join("status.json"),
        &serde_json::to_vec_pretty(&status)?,
        0o644,
    )?;
    // Public status contains no secrets; data remains root-only.
    std::fs::set_permissions(staged, std::fs::Permissions::from_mode(0o755))?;
    Ok(status)
}

fn validate_identity(key: &[u8], cert: &[u8]) -> Result<()> {
    use ed25519_dalek::pkcs8::{DecodePrivateKey, EncodePrivateKey};
    use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
    let key = ed25519_dalek::SigningKey::from_pkcs8_pem(
        std::str::from_utf8(key).context("Invalid device identity")?,
    )
    .context("Invalid device identity")?;
    let der = key.to_pkcs8_der().context("Invalid device identity")?;
    let certified = rustls::sign::CertifiedKey::from_der(
        vec![CertificateDer::from(cert.to_vec())],
        PrivatePkcs8KeyDer::from(der.as_bytes().to_vec()).into(),
        &rustls::crypto::aws_lc_rs::default_provider(),
    )
    .context(
        "Device certificate does not match its private key; repair the profile before installation",
    )?;
    // Do not accept a provider that cannot establish key consistency.
    certified.keys_match().context("Invalid device identity")
}

//! Mutual fingerprint-pinning TLS configuration (protocol.md §4.2).
//!
//! Policy: when `allow_unknown=true`, unknown peer certificate fingerprints are
//! allowed (pairing scenario, where security is guaranteed by SPAKE2 + visual
//! fingerprint comparison), otherwise only the known set is trusted.

use crate::error::NetError;
use removent_core::DeviceIdentity;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

struct SharedInner {
    known: HashSet<[u8; 32]>,
}

/// Verifier state shared by both ends.
#[derive(Clone)]
pub struct PinState {
    inner: Arc<Mutex<SharedInner>>,
    allow_unknown: bool,
}

impl PinState {
    pub fn new(known_fps: impl IntoIterator<Item = [u8; 32]>, allow_unknown: bool) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SharedInner {
                known: known_fps.into_iter().collect(),
            })),
            allow_unknown,
        }
    }

    pub fn add_known(&self, fp: [u8; 32]) {
        self.inner.lock().unwrap().known.insert(fp);
    }

    fn decide(&self, der: &[u8]) -> bool {
        let fp: [u8; 32] = Sha256::digest(der).into();
        self.allow_unknown || self.inner.lock().unwrap().known.contains(&fp)
    }
}

fn tls_error(msg: impl Into<String>) -> rustls::Error {
    let inner: std::sync::Arc<dyn std::error::Error + Send + Sync> =
        std::sync::Arc::new(std::io::Error::other(msg.into()));
    rustls::Error::Other(rustls::OtherError(inner))
}

impl std::fmt::Debug for ServerPinVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerPinVerifier").finish()
    }
}

const VERIFY_SCHEMES: &[rustls::SignatureScheme] = &[
    rustls::SignatureScheme::ED25519,
    rustls::SignatureScheme::RSA_PSS_SHA256,
    rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
];

fn sig_algs() -> rustls::crypto::WebPkiSupportedAlgorithms {
    base_provider().signature_verification_algorithms
}

/// Client side: verify the server certificate fingerprint.
pub struct ServerPinVerifier {
    state: PinState,
    supported: Vec<rustls::SignatureScheme>,
    sig_algs: rustls::crypto::WebPkiSupportedAlgorithms,
}

impl ServerPinVerifier {
    pub fn new(state: PinState) -> Self {
        Self {
            state,
            supported: VERIFY_SCHEMES.to_vec(),
            sig_algs: sig_algs(),
        }
    }
}

impl rustls::client::danger::ServerCertVerifier for ServerPinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if self.state.decide(end_entity.as_ref()) {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(tls_error("unknown peer certificate fingerprint"))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.sig_algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.sig_algs)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.supported.clone()
    }
}

/// Server side: require and verify the client certificate fingerprint (mutual TLS).
pub struct ClientPinVerifier {
    state: PinState,
    supported: Vec<rustls::SignatureScheme>,
    sig_algs: rustls::crypto::WebPkiSupportedAlgorithms,
}

impl ClientPinVerifier {
    pub fn new(state: PinState) -> Self {
        Self {
            state,
            supported: VERIFY_SCHEMES.to_vec(),
            sig_algs: sig_algs(),
        }
    }
}

impl std::fmt::Debug for ClientPinVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientPinVerifier").finish()
    }
}

impl rustls::server::danger::ClientCertVerifier for ClientPinVerifier {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
        if self.state.decide(end_entity.as_ref()) {
            Ok(rustls::server::danger::ClientCertVerified::assertion())
        } else {
            Err(tls_error("unknown client certificate fingerprint"))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.sig_algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.sig_algs)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.supported.clone()
    }
}

/// Build a rustls client config from the device identity (TLS 1.3 + fingerprint pinning).
pub fn client_config(
    identity: &DeviceIdentity,
    pin: PinState,
) -> Result<rustls::ClientConfig, NetError> {
    let provider = base_provider();
    let key = ed25519_key_der(identity)?;
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| NetError::Tls(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(ServerPinVerifier::new(pin)))
        .with_client_auth_cert(
            vec![rustls::pki_types::CertificateDer::from(
                identity.cert_der.clone(),
            )],
            key,
        )
        .map_err(|e| NetError::Tls(e.to_string()))?;
    config.alpn_protocols = vec![ALPN_RVP1.to_vec()];
    Ok(config)
}

/// Build a rustls server config from the device identity (requires client certificates, mutual TLS).
pub fn server_config(
    identity: &DeviceIdentity,
    pin: PinState,
) -> Result<rustls::ServerConfig, NetError> {
    let provider = base_provider();
    let key = ed25519_key_der(identity)?;
    let verifier = Arc::new(ClientPinVerifier::new(pin));
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| NetError::Tls(e.to_string()))?
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![rustls::pki_types::CertificateDer::from(
                identity.cert_der.clone(),
            )],
            key,
        )
        .map_err(|e| NetError::Tls(e.to_string()))?;
    config.alpn_protocols = vec![ALPN_RVP1.to_vec()];
    Ok(config)
}

fn ed25519_key_der(
    identity: &DeviceIdentity,
) -> Result<rustls::pki_types::PrivateKeyDer<'static>, NetError> {
    let der = identity
        .private_pkcs8_der()
        .map_err(|e| NetError::Tls(e.to_string()))?;
    Ok(rustls::pki_types::PrivateKeyDer::Pkcs8(
        rustls::pki_types::PrivatePkcs8KeyDer::from(der),
    ))
}

pub const ALPN_RVP1: &[u8] = b"RVP/1";

pub(crate) fn base_provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

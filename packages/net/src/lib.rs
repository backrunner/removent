//! removent-net: QUIC transport, mDNS discovery, SPAKE2 pairing, and session stream wrappers.

pub mod clipboard;
pub mod control_writer;
pub mod discovery;
pub mod error;
pub mod lan_scan;
pub mod pairing;
pub mod session;
pub mod tls;
pub mod transport;
pub mod trust;

pub use discovery::{Advertiser, DeviceEntry, DiscoveryBrowser, DiscoveryProtocol};
pub use error::{NetError, Result};
pub use pairing::{
    PIN_TTL, PairingChallenge, PairingMsg, PairingProof, client_begin, client_confirm_check,
    client_verify, generate_pin, host_on_begin, host_on_begin_with_secret, host_verify,
};
pub use quinn;
pub use session::{
    ControlCodec, ControlItem, ControlSink, ControlSource, PairCodec, RvpConnection,
};
pub use tls::{ALPN_RVP1, PinState};
pub use transport::{make_client_endpoint, make_server_endpoint};

pub use trust::{CertificateConfirmation, confirm_quic_peer};

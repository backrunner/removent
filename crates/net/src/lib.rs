//! removent-net: QUIC transport, mDNS discovery, SPAKE2 pairing, and session stream wrappers.

pub mod discovery;
pub mod error;
pub mod pairing;
pub mod session;
pub mod tls;
pub mod transport;

pub use discovery::{Advertiser, DeviceEntry, DiscoveryBrowser};
pub use error::{NetError, Result};
pub use pairing::{
    PIN_TTL, PairingMsg, client_begin, client_confirm_check, client_verify, generate_pin,
    host_on_begin, host_verify,
};
pub use quinn;
pub use session::{
    ControlCodec, ControlItem, ControlSink, ControlSource, PairCodec, RvpConnection,
};
pub use tls::{ALPN_RVP1, PinState};
pub use transport::{make_client_endpoint, make_server_endpoint};

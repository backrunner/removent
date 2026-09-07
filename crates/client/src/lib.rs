//! removent-client: controller-side session engine.

pub mod connection;
pub mod jitter;
pub mod rdp;
pub mod session;
pub mod vnc;

pub use jitter::{AudioPacketIn, JitterBuffer, PopOutcome};
pub use session::{
    ClientConfig, ClientSession, ConnectError, DecodedFrame, PinRequest, connect_session,
    quick_resume,
};
pub use vnc::{VncError, VncSession, connect_vnc, connect_vnc_with_credentials};

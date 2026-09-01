//! removent-client: controller-side session engine.

pub mod jitter;
pub mod session;

pub use jitter::{AudioPacketIn, JitterBuffer, PopOutcome};
pub use session::{
    ClientConfig, ClientSession, ConnectError, DecodedFrame, PinRequest, connect_session,
    quick_resume,
};

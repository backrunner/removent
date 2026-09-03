//! removent-host: controlled-side service engine.

pub mod admission;
pub mod frame_dedup;
pub mod input_sink;
pub mod runner;
pub mod sender;
pub mod session;
pub mod vnc;

pub use admission::{AdmissionDecision, decide as decide_admission};
pub use frame_dedup::FrameDeduplicator;
pub use input_sink::{
    InputReleaseTracker, InputSink, RealInputSink, RecordedInput, RecorderInputSink, drag_kind_for,
};
pub use runner::{HostCallbacks, HostEvent, HostRunnerConfig, reject_busy, serve_forever};
pub use sender::{SendAction, SendGate};
pub use session::ControlPumpDeps;
pub use session::validate_resume as validate_resume_for;
pub use session::{
    EstablishedSession, HostConfig, HostError, HostInteractions, HostMediaFeeds, serve_connection,
    spawn_audio_loop, spawn_control_pump, spawn_video_loop,
};
pub use vnc::{VncConfig, serve_vnc};

//! removent-core: settings, identity, storage, and logging (no UI, no protocol details).

pub mod adapt;
pub mod clip;
pub mod error;
pub mod flock;
pub mod i18n;
pub mod identity;
pub mod ipc;
pub mod logging;
pub mod paths;
pub mod peers;
pub mod settings;

pub use adapt::{AdaptationController, QualityState, Sample};
pub use clip::{
    ClipSyncState, MemoryClipboard, TextClipboard, apply_incoming as apply_incoming_clip,
    spawn_clipboard_poller,
};
pub use error::{CoreError, Result};
pub use flock::DataDirLock;
pub use i18n::resolve_locale;
pub use identity::DeviceIdentity;
pub use paths::DataPaths;
pub use peers::{PeerRecord, PeersStore};
pub use settings::{AdmissionMode, Language, QualityPreset, Settings, Theme};

/// Application metadata.
pub const APP_NAME: &str = "Removent";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

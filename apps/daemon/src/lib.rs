//! removentd: resident controlled-end daemon.
//!
//! Responsibilities: UDS IPC service (`removent_core::ipc` protocol) + host runner
//! lifecycle management. The lib/bin split lets integration tests drive the server
//! and state directly.

// The bin target (main.rs) invokes this too; the lib target needs its own copy
// because `t!` expands to per-crate generated code.
rust_i18n::i18n!("locales");

pub mod hostmgr;
#[cfg(target_os = "macos")]
pub mod login_window;
#[cfg(target_os = "macos")]
pub mod power;
#[cfg(not(target_os = "macos"))]
#[path = "power_portable.rs"]
pub mod power;
pub mod server;
pub mod state;
pub mod tcc;

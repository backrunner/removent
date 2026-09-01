//! removentd: resident controlled-end daemon.
//!
//! Responsibilities: UDS IPC service (`removent_core::ipc` protocol) + host runner
//! lifecycle management. The lib/bin split lets integration tests drive the server
//! and state directly.

// The bin target (main.rs) invokes this too; the lib target needs its own copy
// because `t!` expands to per-crate generated code.
rust_i18n::i18n!("locales");

pub mod hostmgr;
pub mod server;
pub mod state;
pub mod tcc;

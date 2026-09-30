//! Client auto-update (release.md §3): GitHub Releases manifest check, signed
//! download, triple verification (sha256 + ed25519 + codesign), atomic .app swap
//! and relaunch.
//!
//! All network I/O shells out to `curl` (always present on macOS; the project
//! already shells out for `open`/`pgrep`) so no HTTP dependency is added.
//! Blocking work runs on the tokio blocking pool; the UI follows progress via
//! `UiEvent::UpdateStatus`.

use crate::engine::UiEvent;
use ed25519_dalek::VerifyingKey;
use removent_core::{DataPaths, Settings, UpdateChannel};
use rust_i18n::t;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Stable manifest URL (the `latest.json` asset of the newest GitHub Release).
pub const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/backrunner/removent/releases/latest/download/latest.json";
const RELEASES_API: &str = "https://api.github.com/repos/backrunner/removent/releases";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatePolicy {
    channel: UpdateChannel,
    endpoint: String,
}

impl UpdatePolicy {
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            channel: settings.update_channel,
            endpoint: settings.update_endpoint.trim().into(),
        }
    }
}

/// Developer ID team used for the official distribution. Never accept ad-hoc
/// signatures or another developer's valid signature as an official update.
const EXPECTED_TEAM_ID: &str = "PB8H83VL3Z";

/// Update manifest contract (`latest.json`, shared with the release pipeline —
/// field names must not change).
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub notes: String,
    /// Release timestamp (contract field; not displayed in the UI yet).
    #[serde(default)]
    #[allow(dead_code)]
    pub pub_date: String,
    pub min_compatible_proto: u16,
    pub signature: String,
}

/// Update state machine snapshot (release.md §3.2):
/// Idle → Checking → Downloading → Verifying → ReadyToInstall → Swapping →
/// Relaunching; any failure lands in `Failed` with a user-facing reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    Idle,
    Checking,
    /// Check finished, no newer version.
    UpToDate,
    /// A newer, signature-valid version exists (prompt only, never forced).
    Available {
        version: String,
        notes: String,
    },
    Downloading {
        version: String,
    },
    Verifying {
        version: String,
    },
    /// Downloaded and triple-verified; waiting to swap (a running session
    /// defers the install until it ends).
    ReadyToInstall {
        version: String,
    },
    Swapping {
        version: String,
    },
    Relaunching,
    Failed(String),
}

impl UpdateStatus {
    /// True while a check/download/verify/swap is in flight (buttons disabled).
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Checking
                | Self::Downloading { .. }
                | Self::Verifying { .. }
                | Self::Swapping { .. }
                | Self::Relaunching
        )
    }
}

/// Shared updater state (engine ↔ worker threads).
pub struct UpdateShared {
    pub status: UpdateStatus,
    policy: UpdatePolicy,
    generation: u64,
    /// Manifest of the available update (Some while Available or later).
    manifest: Option<UpdateManifest>,
    /// Verified, unpacked .app staged in the update cache, ready for the swap.
    staged_app: Option<PathBuf>,
}

impl UpdateShared {
    pub fn new(settings: &Settings) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            status: UpdateStatus::Idle,
            policy: UpdatePolicy::from_settings(settings),
            generation: 0,
            manifest: None,
            staged_app: None,
        }))
    }

    /// Called under the same lock as worker state transitions. Switching a
    /// channel invalidates even an already staged update from the old channel.
    pub fn change_policy(&mut self, policy: UpdatePolicy) -> Result<(), String> {
        if !self.validate_policy_change(&policy)? {
            return Ok(());
        }
        self.policy = policy;
        self.generation = self.generation.wrapping_add(1);
        self.manifest = None;
        self.staged_app = None;
        self.status = UpdateStatus::Idle;
        Ok(())
    }

    pub fn validate_policy_change(&self, policy: &UpdatePolicy) -> Result<bool, String> {
        let changed = self.policy != *policy;
        if changed && self.status.is_busy() {
            return Err(t!("update.err.busy").to_string());
        }
        Ok(changed)
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }
}

fn set_status(
    shared: &Arc<Mutex<UpdateShared>>,
    events: &std::sync::mpsc::Sender<UiEvent>,
    status: UpdateStatus,
) {
    shared.lock().unwrap().status = status.clone();
    let _ = events.send(UiEvent::UpdateStatus(status));
}

// ---- pure logic (unit-tested) ----

mod check;
mod download;
mod fetch;
mod install;
/// Strict SemVer, including prerelease precedence and ignoring build metadata.
mod manifest;
#[cfg(test)]
mod tests;
mod verify;

pub use check::run_check;
#[cfg(test)]
use check::{GithubRelease, fetch_update, release_candidate};
pub use download::run_download;
use download::{DaemonReq, bundle_version};
use fetch::curl_get;
#[cfg(test)]
use fetch::{decode_fetch_output, fetch_with_retry};
pub use install::cleanup_stale_backup;

#[cfg(test)]
use install::replace_bundle;
pub use install::run_install;
use manifest::eligible_upgrade;
pub use manifest::{is_newer, parse_version, release_verifying_key, verify_manifest_signature};
#[cfg(test)]
use verify::release_codesign_requirement;
use verify::{codesign_verify, curl_download, find_app_bundle, sha256_file};

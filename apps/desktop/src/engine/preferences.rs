use super::*;

impl Engine {
    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    pub fn start_discovery(&self) {
        let my_fp = self.fingerprint_short();
        if my_fp.is_empty() {
            tracing::warn!("own fingerprint unavailable; LAN discovery cannot filter this host");
        }
        for protocol in ConnectionProtocol::ALL {
            self.rt.spawn(crate::discovery::run(
                protocol,
                self.discovery_settings.subscribe(),
                my_fp.clone(),
                self.events_tx.clone(),
            ));
        }
    }

    /// Update and persist settings; pushes ReloadSettings to the daemon when online.
    /// Returns Err when the save fails so the UI can surface it.
    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) -> Result<(), String> {
        {
            let mut s = self.settings.lock().unwrap();
            // The tray/CLI may change host_enabled while this window is open.
            // Share Settings::update's file lock, and validate the source change
            // before saving so a busy updater cannot leave disk and memory split.
            let _file_lock = removent_core::DataDirLock::acquire_blocking(
                &self.paths.root.join(".settings.lock"),
            )
            .map_err(|e| e.to_string())?;
            let mut next = Settings::load(&self.paths).map_err(|e| e.to_string())?;
            f(&mut next);
            let mut update = self.update.lock().unwrap();
            let policy = updater::UpdatePolicy::from_settings(&next);
            let changed = update.validate_policy_change(&policy)?;
            next.save(&self.paths).map_err(|e| {
                tracing::error!(err=%e, "settings save failed");
                e.to_string()
            })?;
            update.change_policy(policy)?;
            *s = next;
            if changed {
                let _ = self
                    .events_tx
                    .send(UiEvent::UpdateStatus(update.status.clone()));
            }
            self.discovery_settings.send_if_modified(|current| {
                if *current == s.discovery {
                    return false;
                }
                *current = s.discovery;
                true
            });
        }
        // Reload on the daemon side (the runner picks up the new settings on next restart).
        if let Some(tx) = self.daemon_req.lock().unwrap().as_ref() {
            let _ = tx.send(IpcRequest::ReloadSettings);
        }
        Ok(())
    }

    pub fn data_dir(&self) -> std::path::PathBuf {
        self.paths.root.clone()
    }

    // ---- auto-update (release.md §3) ----

    /// Current update state machine snapshot.
    pub fn update_status(&self) -> UpdateStatus {
        self.update.lock().unwrap().status.clone()
    }

    /// Persist before changing the active policy. Holding the worker lock
    /// prevents a queued download/install from claiming the old candidate.
    pub fn set_update_channel(&self, channel: removent_core::UpdateChannel) -> Result<(), String> {
        self.update_settings(|s| s.update_channel = channel)
    }

    /// Kick a manifest check (the settings-page button; always runs unless a
    /// download/install is already in flight).
    pub fn check_for_updates(&self) {
        let shared = self.update.clone();
        let events = self.events_tx.clone();
        self.rt
            .spawn_blocking(move || updater::run_check(true, shared, events));
    }

    /// Download and triple-verify the available update (lands in ReadyToInstall).
    pub fn download_update(&self) {
        let generation = self.update.lock().unwrap().generation();
        let paths = self.paths.clone();
        let shared = self.update.clone();
        let events = self.events_tx.clone();
        self.rt
            .spawn_blocking(move || updater::run_download(generation, paths, shared, events));
    }

    /// Swap in the staged update and relaunch. Refused while a session is
    /// running (the user must end it first); on success the process exits.
    pub fn install_update(&self) -> std::result::Result<(), String> {
        if self.session_active() {
            return Err(t!("update.err.session_active").to_string());
        }
        let generation = self.update.lock().unwrap().generation();
        let shared = self.update.clone();
        let events = self.events_tx.clone();
        let daemon_req = self.daemon_req.clone();
        self.rt
            .spawn_blocking(move || updater::run_install(generation, shared, events, daemon_req));
        Ok(())
    }

    /// True while a controlling-side session (viewer open) or a controlled-side
    /// peer session is running — installs are deferred until it ends.
    pub fn session_active(&self) -> bool {
        let client_live = self
            .client_task
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|t| !t.is_finished());
        client_live || self.host_sessions.load(Ordering::SeqCst) > 0
    }

    /// Check schedule (release.md §3.1): once 30s after launch, then every 24h;
    /// skipped when the user disabled update checks.
    pub(super) fn spawn_update_scheduler(&self) {
        if std::env::var("REMOVENT_NO_UPDATE_CHECK").as_deref() == Ok("1") {
            return;
        }
        let settings = self.settings.clone();
        let shared = self.update.clone();
        let events = self.events_tx.clone();
        self.rt.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            loop {
                let enabled = settings.lock().unwrap().update_check_enabled;
                if enabled {
                    let shared = shared.clone();
                    let events = events.clone();
                    tokio::task::spawn_blocking(move || {
                        updater::run_check(false, shared, events);
                    });
                }
                tokio::time::sleep(std::time::Duration::from_secs(24 * 60 * 60)).await;
            }
        });
    }
}

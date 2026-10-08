use super::*;

impl Engine {
    pub fn refresh_host_status(&self) {
        if let Some(tx) = self.daemon_req.lock().unwrap().as_ref() {
            let _ = tx.send(IpcRequest::Status);
        }
    }

    pub fn request_host_permission(&self, permission: removent_core::ipc::HostPermission) {
        let sent = self.daemon_req.lock().unwrap().as_ref().is_some_and(|tx| {
            tx.send(IpcRequest::RequestPermission { permission })
                .is_ok()
        });
        if !sent {
            let _ = self
                .events_tx
                .send(UiEvent::Notice(t!("status.daemon_offline").to_string()));
        }
    }

    pub fn generate_pairing_code(&self) {
        if let Some(tx) = self.daemon_req.lock().unwrap().as_ref() {
            let _ = tx.send(IpcRequest::PairingGenerate);
        } else {
            let _ = self
                .events_tx
                .send(UiEvent::Notice(t!("pairing.daemon_required").to_string()));
        }
    }
    pub fn revoke_pairing_code(&self) {
        if let Some(tx) = self.daemon_req.lock().unwrap().as_ref() {
            let _ = tx.send(IpcRequest::PairingRevoke);
        }
    }
    /// Enable/disable the host service; tries to spawn the daemon first when offline.
    pub fn set_host_enabled(&self, on: bool) {
        if let Some(tx) = self.daemon_req.lock().unwrap().as_ref() {
            let _ = tx.send(IpcRequest::SetEnabled { on });
            return;
        }
        // launchctl readiness can take seconds; never wait on the UI thread.
        let engine = self.clone();
        let slot = self.daemon_req.clone();
        let events = self.events_tx.clone();
        self.rt.spawn(async move {
            let result = tokio::task::spawn_blocking(move || engine.spawn_daemon_process()).await;
            if let Err(e) = result.map_err(anyhow::Error::from).and_then(|v| v) {
                let _ = events.send(UiEvent::Notice(
                    t!("notice.daemon_spawn_failed", err = format!("{e:#}")).to_string(),
                ));
                return;
            }
            for _ in 0..25 {
                if let Some(tx) = slot.lock().unwrap().as_ref() {
                    let _ = tx.send(IpcRequest::SetEnabled { on });
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
            let _ = events.send(UiEvent::Notice(
                t!("notice.daemon_start_timeout").to_string(),
            ));
        });
    }

    /// Start IPC/hosting from the saved settings without changing the service
    /// switch. Explicit stop intent survives reopening the desktop.
    pub fn start_background_daemon(&self) {
        let Some(bin) = Self::daemon_binary() else {
            return;
        };
        if std::env::var("REMOVENT_DEV_SUPERVISED").as_deref() == Ok("1") {
            return;
        }
        let engine = self.clone();
        self.rt.spawn_blocking(move || {
            #[cfg(target_os = "macos")]
            let result = removent_core::service::Service::new(engine.paths.clone(), bin)
                .and_then(|service| service.ensure_running());
            #[cfg(not(target_os = "macos"))]
            let result = engine.spawn_daemon_process();
            if let Err(e) = result {
                let _ = engine.events_tx.send(UiEvent::Notice(
                    t!("notice.daemon_spawn_failed", err = format!("{e:#}")).to_string(),
                ));
            }
        });
    }

    /// Admission decision reply (forwarded to the daemon).
    pub fn answer_admission(&self, request_id: u64, allow: bool) {
        if let Some(tx) = self.daemon_req.lock().unwrap().as_ref() {
            let _ = tx.send(IpcRequest::AdmissionReply { request_id, allow });
        }
    }

    /// Locate the removentd executable: same directory as the current process
    /// (app bundle / debug dir) first, then PATH.
    pub(super) fn daemon_binary() -> Option<std::path::PathBuf> {
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            let candidate = dir.join("removentd");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths).find_map(|dir| {
                let candidate = dir.join("removentd");
                candidate.is_file().then_some(candidate)
            })
        })
    }

    pub(super) fn spawn_daemon_process(&self) -> Result<()> {
        let bin = Self::daemon_binary()
            .ok_or_else(|| anyhow::anyhow!(t!("notice.daemon_binary_missing").to_string()))?;
        #[cfg(target_os = "macos")]
        if std::env::var("REMOVENT_DEV_SUPERVISED").as_deref() != Ok("1") {
            return removent_core::service::Service::new(self.paths.clone(), bin)?.start();
        }
        let mut child = std::process::Command::new(bin)
            .arg("--background")
            .env("REMOVENT_DATA_DIR", &self.paths.root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        // Reap the child when it exits: dropping the handle without wait() would
        // leave a zombie process.
        std::thread::Builder::new()
            .name("removentd-reaper".into())
            .spawn(move || match child.wait() {
                Ok(status) => tracing::info!(?status, "removentd exited"),
                Err(e) => tracing::warn!(err=%e, "removentd wait failed"),
            })
            .expect("spawn removentd reaper");
        tracing::info!("removentd spawned by app");
        Ok(())
    }

    /// Long-lived daemon IPC link: connect, forward events, poll status periodically,
    /// reconnect on drop.
    pub(super) fn spawn_daemon_link(&self) {
        let paths = self.paths.clone();
        let events = self.events_tx.clone();
        let slot = self.daemon_req.clone();
        let online = self.daemon_online.clone();
        let running = self.host_running.clone();
        let perms = self.daemon_perms.clone();
        let sessions = self.host_sessions.clone();
        self.rt.spawn(async move {
            loop {
                match removent_core::ipc::connect(&paths).await {
                    Ok((mut r, mut w)) => {
                        online.store(true, Ordering::SeqCst);
                        let _ = events.send(UiEvent::DaemonOnline(true));
                        let (req_tx, mut req_rx) =
                            tokio::sync::mpsc::unbounded_channel::<IpcRequest>();
                        *slot.lock().unwrap() = Some(req_tx);
                        // Take a snapshot right after connecting.
                        let _ = removent_core::ipc::write_msg(&mut w, &IpcRequest::Status).await;
                        let mut reader = removent_core::ipc::MessageReader::default();
                        let mut tick = tokio::time::interval(std::time::Duration::from_secs(3));
                        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        loop {
                            tokio::select! {
                                line = reader.read::<_, serde_json::Value>(&mut r) => {
                                    match line {
                                        Ok(Some(v)) => handle_daemon_message(
                                            v, &events, &running, &perms, &sessions,
                                        ),
                                        Ok(None) => break, // EOF
                                        Err(e) => {
                                            tracing::warn!(err=%e, "daemon ipc read error");
                                            break;
                                        }
                                    }
                                }
                                Some(req) = req_rx.recv() => {
                                    if removent_core::ipc::write_msg(&mut w, &req).await.is_err() {
                                        break;
                                    }
                                }
                                _ = tick.tick() => {
                                    if removent_core::ipc::write_msg(&mut w, &IpcRequest::Status)
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                            }
                        }
                        slot.lock().unwrap().take();
                        online.store(false, Ordering::SeqCst);
                        running.store(false, Ordering::SeqCst);
                        sessions.store(0, Ordering::SeqCst);
                        let _ = events.send(UiEvent::DaemonOnline(false));
                    }
                    Err(_) => {
                        online.store(false, Ordering::SeqCst);
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        });
    }

    // ---- client sessions ----
}

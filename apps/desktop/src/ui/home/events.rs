use super::*;

impl HomeView {
    pub(super) fn handle_event(
        &mut self,
        ev: UiEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !ev.belongs_to_client(self.engine.client_generation()) {
            return;
        }
        match ev {
            UiEvent::DeviceFound {
                fp,
                name,
                addr,
                protocol,
            } => {
                // A queued resolve must not reinsert a row after its toggle was turned off.
                if crate::discovery::enabled(self.engine.settings().discovery, protocol) {
                    self.devices.insert(
                        fp,
                        DeviceRow {
                            name,
                            addr,
                            protocol,
                        },
                    );
                }
            }
            UiEvent::DeviceLost(fp) => {
                self.devices.remove(&fp);
                if matches!(&self.selected, Some(Selection::Device(s)) if s == &fp) {
                    self.selected = None;
                }
            }
            UiEvent::PairingPin(pin) => {
                if matches!(self.pin_dialog, Some(PinDialog::Entry(_))) {
                    // The controlling side is entering a PIN: do not cover the entry dialog,
                    // just hint.
                    self.set_status(t!("status.pairing_busy").to_string(), StatusTone::Info);
                } else {
                    // The Display dialog auto-closes after 300s (PIN TTL); only clear it if it
                    // still shows the same PIN by then.
                    let pin_timer = pin.clone();
                    cx.spawn(async move |this: gpui::WeakEntity<HomeView>, cx| {
                        cx.background_executor()
                            .timer(Duration::from_secs(300))
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            if matches!(&this.pin_dialog, Some(PinDialog::Display(p)) if p == &pin_timer)
                            {
                                this.pin_dialog = None;
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                    self.dialog_seq += 1;
                    self.pin_dialog = Some(PinDialog::Display(pin));
                }
            }
            UiEvent::PairingDone(peer_name) => {
                // Only clear the controlled side's Display dialog; the controlling Entry is
                // wrapped up by SessionReady.
                if matches!(self.pin_dialog, Some(PinDialog::Display(_))) {
                    self.pin_dialog = None;
                }
                self.trusted = self.engine.trusted_short_fps();
                self.set_status(t!("status.pairing_done", peer = peer_name), StatusTone::Ok);
            }
            UiEvent::AdmissionRequest {
                request_id,
                peer_name,
                peer_fp_short,
            } => {
                // Automated-testing escape hatch: REMOVENT_AUTO_ADMIT=1 auto-allows.
                if std::env::var("REMOVENT_AUTO_ADMIT").as_deref() == Ok("1") {
                    self.engine.answer_admission(request_id, true);
                    self.set_status(
                        t!("status.auto_admitted", peer = peer_name),
                        StatusTone::Warn,
                    );
                } else {
                    // A pending dialog already exists: auto-deny the old request before
                    // replacing it, so the old one is not left hanging after being covered.
                    if let Some(old) = self.admission.take() {
                        self.engine.answer_admission(old.request_id, false);
                    }
                    self.admission = Some(PendingAdmission {
                        request_id,
                        peer_name,
                        peer_fp_short,
                    });
                    // UI-side fallback: the daemon resolves the request after 30s, but if
                    // it dies while the request is pending no AdmissionResolved ever
                    // arrives — close the dialog a few seconds past the deadline.
                    cx.spawn(async move |this: gpui::WeakEntity<HomeView>, cx| {
                        cx.background_executor()
                            .timer(Duration::from_secs(35))
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            if this.admission.as_ref().map(|a| a.request_id) == Some(request_id) {
                                this.admission = None;
                                this.set_status(
                                    t!("status.admission_expired").to_string(),
                                    StatusTone::Warn,
                                );
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                }
            }
            UiEvent::ConnectionProgress { stage, .. } => {
                self.set_connection_stage(stage, cx);
            }
            UiEvent::ClientNeedsPin { tx, .. } => {
                self.set_connection_stage(ConnectionStage::Pairing, cx);
                self.dialog_seq += 1;
                self.pin_dialog = Some(PinDialog::Entry(tx));
                // Start from an empty field and hand it the keyboard focus.
                self.clear_pin_input(window, cx);
                let focus = self.pin_input.focus_handle(cx);
                window.focus(&focus);
            }
            UiEvent::SessionReady { codec, .. } => {
                self.connection_dialog = None;
                self.connection_subscription = None;
                // Close only the controlling side's Entry dialog: dropping the oneshot here
                // is safe (the client pairing task has been aborted). A Display dialog
                // (we are showing a PIN to someone else) must survive.
                if matches!(self.pin_dialog, Some(PinDialog::Entry(_))) {
                    self.pin_dialog = None;
                    self.clear_pin_input(window, cx);
                }
                // Controlling side: session established, open the viewer window.
                if let Some(name) = self.connecting.take() {
                    let open_result = self
                        .engine
                        .take_client_frames()
                        .ok_or_else(|| t!("err.frame_channel_missing").to_string())
                        .and_then(|rx| {
                            viewer::open_viewer_window(self.engine.clone(), rx, name.clone(), cx)
                        });
                    match open_result {
                        Ok(()) => self.set_status(
                            t!("status.session_active", name = name, codec = codec),
                            StatusTone::Ok,
                        ),
                        Err(e) => {
                            // Hang up the headless session so the connection does not dangle
                            // without a viewer.
                            self.engine.disconnect_client();
                            self.set_status(
                                t!("status.viewer_open_failed", err = e),
                                StatusTone::Err,
                            )
                        }
                    }
                } else {
                    self.set_status(t!("status.peer_connected", codec = codec), StatusTone::Ok);
                }
            }
            UiEvent::ConnectFailed { error: e, .. } => {
                if let Some(dialog) = &self.connection_dialog {
                    dialog.update(cx, |form, cx| {
                        form.set_connecting(
                            false,
                            Some(t!("status.connect_failed", err = e.clone()).to_string()),
                            cx,
                        )
                    });
                }
                // There is no SessionClosed fallback after a failure; this must reset by itself.
                self.connecting = None;
                // A zombie Entry dialog is useless here: its oneshot peer died with the
                // client task, so submitting would silently go nowhere.
                if matches!(self.pin_dialog, Some(PinDialog::Entry(_))) {
                    self.pin_dialog = None;
                    self.clear_pin_input(window, cx);
                }
                self.set_status(t!("status.connect_failed", err = e), StatusTone::Err);
            }
            UiEvent::SessionClosed {
                generation,
                reason: r,
            } => {
                if self.cancelled_generation == Some(generation) {
                    self.cancelled_generation = None;
                    return;
                }
                if let Some(dialog) = &self.connection_dialog {
                    dialog.update(cx, |form, cx| form.set_connecting(false, None, cx));
                }
                self.connecting = None;
                // Same zombie-Entry cleanup as ConnectFailed.
                if matches!(self.pin_dialog, Some(PinDialog::Entry(_))) {
                    self.pin_dialog = None;
                    self.clear_pin_input(window, cx);
                }
                // Keep the red connect-failure message just shown from being overwritten by
                // the subsequent close event.
                if !matches!(self.status_tone, StatusTone::Err) {
                    self.set_status(t!("status.session_ended", reason = r), StatusTone::Info);
                }
            }
            UiEvent::HostSessionStarted { peer_name, codec } => {
                self.set_status(
                    t!(
                        "status.host_session_started",
                        peer = peer_name,
                        codec = codec
                    ),
                    StatusTone::Ok,
                );
            }
            UiEvent::HostSessionEnded(reason) => {
                self.set_status(
                    t!("status.host_session_ended", reason = reason),
                    StatusTone::Info,
                );
            }
            UiEvent::AdmissionResolved { request_id, allow } => {
                let was_pending = self.admission.as_ref().map(|a| a.request_id) == Some(request_id);
                if was_pending {
                    self.admission = None;
                }
                // The daemon also broadcasts a resolved(deny) after the user answered
                // locally: only show the timeout copy when the request was still pending,
                // otherwise it would overwrite the "denied" message just shown.
                if !allow && was_pending {
                    self.set_status(t!("status.admission_expired").to_string(), StatusTone::Warn);
                }
            }
            UiEvent::CloudSync { status, entries } => {
                self.cloud_sync_status = status;
                self.saved = entries;
                if matches!(&self.selected, Some(Selection::Saved(id)) if !self.saved.iter().any(|e| &e.id == id))
                {
                    self.selected = None;
                }
            }
            UiEvent::Notice(msg) => {
                self.set_status(msg, StatusTone::Warn);
            }
            UiEvent::HostStateChanged(on) => {
                self.host_on = on;
                self.set_status(
                    if on {
                        t!("status.host_running").to_string()
                    } else {
                        t!("status.host_stopped").to_string()
                    },
                    if on { StatusTone::Ok } else { StatusTone::Info },
                );
            }
            UiEvent::DaemonOnline(on) => {
                self.daemon_online = on;
                if !on {
                    self.host_on = false;
                    self.daemon_perms = None;
                    // A dead daemon can no longer resolve a pending admission request;
                    // drop it so the dialog does not hang forever.
                    self.admission = None;
                    self.set_status(t!("status.daemon_offline").to_string(), StatusTone::Warn);
                }
            }
            UiEvent::DaemonPermissions {
                screen_recording,
                accessibility,
            } => {
                self.daemon_perms = Some((screen_recording, accessibility));
            }
            UiEvent::UpdateStatus(st) => {
                // Events are wakeups: a channel switch may already have
                // invalidated a queued Available/ReadyToInstall snapshot.
                if st != self.engine.update_status() {
                    return;
                }
                // Download+verify finished: install straight away when no session
                // is running; otherwise wait for the user to end the session and
                // click "Install and Relaunch" (release.md §3: never forced).
                let auto_install = matches!(&st, UpdateStatus::ReadyToInstall { .. })
                    && !self.engine.session_active();
                self.update_status = st;
                if auto_install {
                    let _ = self.engine.install_update();
                }
            }
        }
        cx.notify();
    }

    // ---- rendering ----
}

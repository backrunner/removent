use super::*;

impl HomeView {
    pub(super) fn set_status(&mut self, text: impl Into<String>, tone: StatusTone) {
        self.status = text.into();
        self.status_tone = tone;
    }

    // ---- events and actions ----

    pub(super) fn toggle_host(&mut self, cx: &mut Context<Self>) {
        // The switch is forwarded to the daemon; the result is filled back via the
        // HostStateChanged event.
        let on = !self.host_on;
        self.engine.set_host_enabled(on);
        self.set_status(
            if on {
                if self.daemon_online {
                    t!("status.host_starting").to_string()
                } else {
                    t!("status.daemon_starting").to_string()
                }
            } else {
                t!("status.host_stopping").to_string()
            },
            StatusTone::Info,
        );
        cx.notify();
    }

    pub(super) fn connect_device(&mut self, fp: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.connecting.is_some() {
            self.set_status(t!("status.connecting_other").to_string(), StatusTone::Warn);
            cx.notify();
            return;
        }
        let Some(row) = self.devices.get(fp).cloned() else {
            return;
        };
        if row.protocol == ConnectionProtocol::Removent {
            self.start_connect(row.name.clone(), row.addr, cx);
        } else if self.connection_dialog.is_none() {
            self.open_connection_dialog(None, window, cx);
            if let Some(dialog) = &self.connection_dialog {
                dialog.update(cx, |form, cx| {
                    form.prefill_discovered(row.protocol, row.name, row.addr, window, cx)
                });
            }
        }
    }

    pub(super) fn start_connect(&mut self, name: String, addr: SocketAddr, cx: &mut Context<Self>) {
        match self.engine.connect_to(addr) {
            Ok(()) => {
                self.connecting = Some(name.clone());
                self.connection_stage = ConnectionStage::Connecting;
                self.set_status(t!("status.connecting", name = name), StatusTone::Info);
            }
            Err(e) => self.set_status(
                t!("status.connect_failed", err = format!("{e:#}")),
                StatusTone::Err,
            ),
        }
        cx.notify();
    }

    /// Reconnect from a bookmark. Passwords come back from the Keychain; when a
    /// credentialed entry has no stored secret left, the prefilled form opens
    /// instead so the user can re-enter it.
    pub(super) fn connect_saved(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.connecting.is_some() || self.connection_save_task.is_some() {
            self.set_status(t!("status.connecting_other").to_string(), StatusTone::Warn);
            cx.notify();
            return;
        }
        self.saved = self.engine.saved_connections();
        let Some(entry) = self.saved.iter().find(|s| s.id == id).cloned() else {
            return;
        };
        let mut request = entry.to_request();
        if entry.needs_credentials() {
            match self.engine.saved_password(&entry) {
                Some(password) => request.password = password,
                None => {
                    self.open_connection_dialog(Some(&entry), window, cx);
                    return;
                }
            }
        }
        let name = entry.display_name();
        self.engine.touch_saved_connection(&entry.id);
        match self.engine.connect_request(request) {
            Ok(()) => {
                self.connecting = Some(name.clone());
                self.connection_stage = ConnectionStage::Connecting;
                self.set_status(t!("status.connecting", name = name), StatusTone::Info);
            }
            Err(e) => self.set_status(
                t!("status.connect_failed", err = format!("{e:#}")),
                StatusTone::Err,
            ),
        }
        cx.notify();
    }

    pub(super) fn remove_saved(&mut self, id: &str, cx: &mut Context<Self>) {
        let engine = self.engine.clone();
        let id = id.to_owned();
        let operation_id = id.clone();
        let remove = cx.background_executor().spawn(async move {
            let result = engine.remove_saved_connection(&operation_id);
            (result, engine.saved_connections())
        });
        cx.spawn(async move |this, cx| {
            let (result, saved) = remove.await;
            let _ = this.update(cx, |this, cx| {
                this.saved = saved;
                if !this.saved.iter().any(|entry| entry.id == id)
                    && matches!(&this.selected, Some(Selection::Saved(s)) if s == &id)
                {
                    this.selected = None;
                }
                match result {
                    Ok(()) => this.set_status(
                        t!("status.connection_removed").to_string(),
                        StatusTone::Info,
                    ),
                    Err(e) => this.set_status(
                        t!("status.connection_save_failed", err = e.to_string()),
                        StatusTone::Err,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn cancel_connection(&mut self, cx: &mut Context<Self>) {
        // Dropping the UI task prevents a completed disk/Keychain operation
        // from starting a connection after Cancel. No live session is stopped.
        if self.connection_save_task.take().is_some() {
            if let Some(dialog) = &self.connection_dialog {
                dialog.update(cx, |form, cx| form.cancel(cx));
            }
            self.set_status(t!("connection.cancelled").to_string(), StatusTone::Info);
            cx.notify();
            return;
        }
        if self.connecting.take().is_some() {
            self.engine.disconnect_client();
            self.cancelled_generation = Some(self.engine.client_generation());
            if let Some(dialog) = &self.connection_dialog {
                dialog.update(cx, |form, cx| form.cancel(cx));
            }
            self.set_status(t!("connection.cancelled").to_string(), StatusTone::Info);
            cx.notify();
        }
    }

    pub(super) fn set_connection_stage(&mut self, stage: ConnectionStage, cx: &mut Context<Self>) {
        if self.connecting.is_some() {
            self.connection_stage = stage;
            if let Some(dialog) = &self.connection_dialog {
                dialog.update(cx, |form, cx| form.set_stage(stage, cx));
            }
        }
    }
}

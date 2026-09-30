use super::*;

impl HomeView {
    pub(super) fn open_connection_dialog(
        &mut self,
        prefill: Option<&SavedConnection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.connection_dialog.is_some()
            || self.connecting.is_some()
            || self.pin_dialog.is_some()
            || self.admission.is_some()
        {
            return;
        }
        let dialog = cx.new(|cx| ConnectionDialog::new(window, cx));
        dialog.update(cx, |form, _| form.set_relay_choices(self.saved.clone()));
        if let Some(saved) = prefill {
            let password = self.engine.saved_password(saved);
            dialog.update(cx, |form, cx| form.prefill(saved, password, window, cx));
        }
        self.connection_subscription = Some(cx.subscribe_in(
            &dialog,
            window,
            |this, dialog, event: &ConnectionDialogEvent, window, cx| {
                if this.pin_dialog.is_some() || this.admission.is_some() {
                    return;
                }
                match event {
                    ConnectionDialogEvent::Close => {
                        this.cancel_connection(cx);
                        this.connection_dialog = None;
                        this.connection_subscription = None;
                        window.focus(&this.focus);
                    }
                    ConnectionDialogEvent::Cancel => {
                        this.cancel_connection(cx);
                        window.focus(&dialog.focus_handle(cx));
                    }
                    ConnectionDialogEvent::Submit | ConnectionDialogEvent::Save => {
                        if this.connecting.is_some() || this.connection_save_task.is_some() {
                            return;
                        }
                        let Ok(request) = dialog.read(cx).request(cx) else {
                            return;
                        };
                        let memo = dialog.read(cx).memo_name(cx);
                        let id = dialog.read(cx).saved_id().map(str::to_owned);
                        let save_only = *event == ConnectionDialogEvent::Save;
                        let engine = this.engine.clone();
                        let save_request = request;
                        let save_name = memo.clone();
                        dialog.update(cx, |form, cx| {
                            form.set_connecting(true, None, cx);
                            form.set_save_warning(None, cx);
                        });
                        // Keychain and cross-process file locks must never park GPUI.
                        let save = cx.background_executor().spawn(async move {
                            let result =
                                engine.save_connection(&save_request, save_name, id.as_deref());
                            (result, engine.saved_connections(), save_request)
                        });
                        let dialog = dialog.clone();
                        this.connection_save_task =
                            Some(cx.spawn_in(window, async move |this, cx| {
                                let (result, saved, request) = save.await;
                                let _ = this.update_in(cx, |this, window, cx| {
                                    this.connection_save_task = None;
                                    if this.connection_dialog.as_ref() != Some(&dialog) {
                                        return;
                                    }
                                    this.save_warning = None;
                                    match result {
                                        Ok(entry) => {
                                            this.selected =
                                                Some(Selection::Saved(entry.id.clone()));
                                            dialog
                                                .update(cx, |form, _| form.set_saved_id(entry.id));
                                        }
                                        Err(e) => {
                                            this.save_warning = Some(
                                                t!(
                                                    "status.connection_save_failed",
                                                    err = e.to_string()
                                                )
                                                .to_string(),
                                            )
                                        }
                                    }
                                    this.saved = saved;
                                    dialog.update(cx, |form, cx| {
                                        form.set_save_warning(this.save_warning.clone(), cx)
                                    });
                                    if save_only {
                                        dialog.update(cx, |form, cx| {
                                            form.set_connecting(false, None, cx)
                                        });
                                        if this.save_warning.is_none() {
                                            this.connection_dialog = None;
                                            this.connection_subscription = None;
                                            window.focus(&this.focus);
                                        }
                                        cx.notify();
                                        window.refresh();
                                        return;
                                    }
                                    let name = if memo.is_empty() {
                                        format!(
                                            "{} ({})",
                                            request.address,
                                            request.protocol.label()
                                        )
                                    } else {
                                        memo
                                    };
                                    match this.engine.connect_request(request) {
                                        Ok(()) => {
                                            this.connecting = Some(name.clone());
                                            this.connection_stage = ConnectionStage::Resolving;
                                            this.set_status(
                                                t!("status.connecting", name = name),
                                                StatusTone::Info,
                                            );
                                            dialog.update(cx, |form, cx| {
                                                form.set_connecting(true, None, cx)
                                            });
                                            window.focus(&dialog.focus_handle(cx));
                                        }
                                        Err(error) => dialog.update(cx, |form, cx| {
                                            form.set_connecting(
                                                false,
                                                Some(
                                                    t!(
                                                        "status.connect_failed",
                                                        err = format!("{error:#}")
                                                    )
                                                    .to_string(),
                                                ),
                                                cx,
                                            )
                                        }),
                                    }
                                    cx.notify();
                                    window.refresh();
                                });
                            }));
                    }
                }
                cx.notify();
                window.refresh();
            },
        ));
        self.connection_dialog = Some(dialog);
        self.dialog_seq += 1;
        cx.notify();
    }
}

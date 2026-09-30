use super::*;

impl HomeView {
    pub fn new(engine: Engine, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let events = engine.events_rx.lock().unwrap().take();
        let settings = engine.settings();
        let device_name_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("home.name_placeholder").to_string())
        });
        device_name_input.update(cx, |s, cx| {
            s.set_value(settings.device_name.clone(), window, cx);
        });
        let vnc_password_input = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(t!("settings.vnc_password_placeholder").to_string())
        });
        vnc_password_input.update(cx, |s, cx| {
            s.set_value(settings.vnc_password.clone(), window, cx);
        });
        let focus = cx.focus_handle();
        window.focus(&focus);

        // std::mpsc → async bridge: only wakes the UI when an engine event arrives.
        // Previously this was per-frame polling + full re-render, which kept consuming frames
        // during resize/idle — both laggy and power-hungry.
        let (tx_async, mut rx_async) = futures::channel::mpsc::unbounded::<UiEvent>();
        let bridge_alive = Arc::new(AtomicBool::new(true));
        if let Some(rx) = events {
            let alive = bridge_alive.clone();
            std::thread::Builder::new()
                .name("ui-event-bridge".into())
                .spawn(move || {
                    // recv_timeout instead of recv: when the home window closes while the
                    // engine is still alive (a viewer session is running), a blocking recv
                    // would park this thread forever. The timeout lets it notice the
                    // liveness flag being cleared on HomeView drop and exit.
                    loop {
                        match rx.recv_timeout(Duration::from_secs(1)) {
                            Ok(ev) => {
                                if tx_async.unbounded_send(ev).is_err() {
                                    break;
                                }
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                                if !alive.load(Ordering::Relaxed) {
                                    break;
                                }
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    }
                })
                .expect("spawn ui-event bridge");
        }
        // spawn_in + update_in: obtain the Window handle and refresh the window explicitly
        // after handling an event (child-entity notify does not bubble up to the window root
        // Root automatically; window.refresh() is required).
        cx.spawn_in(window, async move |this: gpui::WeakEntity<HomeView>, cx| {
            use futures::StreamExt;
            while let Some(ev) = rx_async.next().await {
                let r = this.update_in(&mut *cx, |this, window, cx| {
                    this.handle_event(ev, window, cx);
                    window.refresh();
                });
                if let Err(e) = r {
                    eprintln!("[dbg] update_in failed: {e}");
                    break;
                }
            }
        })
        .detach();

        // Re-render on window activation: re-check TCC permission state (the user may have
        // just granted access in System Settings).
        let sub_activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.local_perms = (
                    PermissionKind::ScreenCapture.granted(),
                    PermissionKind::Accessibility.granted(),
                );
                cx.notify();
            }
        });
        // Follow system appearance: when the theme setting is System, switch with the OS
        // light/dark mode.
        let sub_appearance = cx.observe_window_appearance(window, |this, window, cx| {
            if this.engine.settings().theme == ThemePref::System {
                apply_theme_pref(ThemePref::System, window, cx);
            }
            cx.notify();
        });

        let search_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("home.search_placeholder").to_string())
        });
        let pin_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("home.pin_placeholder").to_string())
        });
        let search_sub = cx.subscribe(&search_input, |_, _, _: &InputEvent, cx| cx.notify());
        let pin_sub = cx.subscribe_in(
            &pin_input,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. }
                    if matches!(this.pin_dialog, Some(PinDialog::Entry(_)))
                        && this.admission.is_none() =>
                {
                    this.submit_pin(window, cx)
                }
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        );

        let mut subscriptions = vec![sub_activation, sub_appearance, search_sub, pin_sub];
        for (input, is_name) in [(&device_name_input, true), (&vnc_password_input, false)] {
            subscriptions.push(
                cx.subscribe(input, move |this, _, ev: &InputEvent, cx| match ev {
                    InputEvent::PressEnter { .. }
                        if this.settings_open
                            && this.connection_dialog.is_none()
                            && this.pin_dialog.is_none()
                            && this.admission.is_none() =>
                    {
                        if is_name {
                            this.save_device_name(cx);
                        } else {
                            this.save_vnc(cx);
                        }
                    }
                    InputEvent::Change => cx.notify(),
                    _ => {}
                }),
            );
        }

        Self {
            my_fp_short: engine.fingerprint_short(),
            trusted: engine.trusted_short_fps(),
            update_status: engine.update_status(),
            cloud_sync_status: engine
                .cloud_sync_command(removent_client::cloud_sync::Command::Status)
                .unwrap_or_default(),
            saved: engine.saved_connections(),
            engine,
            _subscriptions: subscriptions,
            devices: BTreeMap::new(),
            selected: None,
            status: t!("status.ready").to_string(),
            status_tone: StatusTone::Info,
            save_warning: None,
            host_on: false,
            daemon_online: false,
            daemon_perms: None,
            local_perms: (
                PermissionKind::ScreenCapture.granted(),
                PermissionKind::Accessibility.granted(),
            ),
            connecting: None,
            connection_stage: ConnectionStage::Resolving,
            cancelled_generation: None,
            admission: None,
            pin_dialog: None,
            pin_input,
            search_input,
            connection_dialog: None,
            connection_subscription: None,
            connection_save_task: None,
            settings_open: false,
            settings_section: 0,
            dialog_seq: 0,
            device_name_input,
            vnc_password_input,
            focus,
            bridge_alive,
        }
    }
}

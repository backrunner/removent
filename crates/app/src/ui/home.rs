//! Home main window: device list + detail panel + pairing/admission dialogs + settings
//! (ui-design §4.1/§4.3/§4.4).
//!
//! Interaction flow: browse/search → single-click to select and view details
//! (double-click connects directly) → session window.
//! Visual principles: no cards inside cards, hairline separators, monospace for all
//! technical info, single entry point per control.

use crate::engine::{Engine, UiEvent};
use crate::permissions::PermissionKind;
use crate::theme;
use crate::ui::motion;
use crate::ui::viewer;
use crate::ui::widgets::*;
use crate::updater::UpdateStatus;
use gpui::{
    Animation, AnimationExt, App, Context, Div, ElementId, Entity, FocusHandle, Focusable, Render,
    Window, div, prelude::*, px, relative,
};
use gpui_component::{
    ActiveTheme, Disableable, Sizable, TitleBar,
    button::{Button, ButtonVariants},
    divider::Divider,
    input::{Input, InputState},
    spinner::Spinner,
    switch::Switch,
    tag::Tag,
};
use removent_core::{AdmissionMode, Language, Theme as ThemePref};
use rust_i18n::t;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::oneshot;

gpui::actions!(home, [HomeEscape]);

#[derive(Clone)]
struct DeviceRow {
    name: String,
    addr: SocketAddr,
}

#[derive(Clone, Copy)]
enum StatusTone {
    Info,
    Ok,
    Warn,
    Err,
}

struct PendingAdmission {
    request_id: u64,
    peer_name: String,
    peer_fp_short: String,
}

enum PinDialog {
    /// Controlled side: display the PIN for the peer to enter.
    Display(String),
    /// Controlling side: enter the PIN shown on the peer's screen.
    Entry(oneshot::Sender<String>),
}

pub struct HomeView {
    engine: Engine,
    _subscriptions: Vec<gpui::Subscription>,
    devices: BTreeMap<String, DeviceRow>,
    selected: Option<String>,
    status: String,
    status_tone: StatusTone,
    host_on: bool,
    daemon_online: bool,
    /// daemon-reported TCC permissions (screen_recording, accessibility); None until
    /// the first StatusReport arrives.
    daemon_perms: Option<(bool, bool)>,
    /// Controlling in progress: waiting for SessionReady to open the viewer (stores the peer name).
    connecting: Option<String>,
    /// Short-fingerprint cache of trusted devices: the render hot path must not hit disk;
    /// refreshed on pairing completion / settings save.
    trusted: HashSet<String>,
    admission: Option<PendingAdmission>,
    pin_dialog: Option<PinDialog>,
    /// Bumped every time a dialog opens: folded into the animation element id so the entry
    /// transition replays on each open (animation state is keyed by id and may survive remount).
    dialog_seq: u64,
    pin_input: Entity<InputState>,
    search_input: Entity<InputState>,
    manual_input: Entity<InputState>,
    show_manual: bool,
    manual_error: Option<String>,
    settings_open: bool,
    device_name_input: Entity<InputState>,
    vnc_username_input: Entity<InputState>,
    vnc_password_input: Entity<InputState>,
    /// Auto-update state machine snapshot (engine → UiEvent::UpdateStatus).
    update_status: UpdateStatus,
    my_fp_short: String,
    focus: FocusHandle,
    /// Liveness flag for the ui-event-bridge thread: cleared on drop so the thread can
    /// exit even when the engine (the channel sender) outlives this window.
    bridge_alive: Arc<AtomicBool>,
}

/// Address display: show only the IP (drop the zone and port).
fn fmt_addr(addr: &SocketAddr) -> String {
    addr.ip().to_string()
}

/// Grouped PIN display: 6 digits → "123 456".
fn group_pin(pin: &str) -> String {
    if pin.len() == 6 {
        format!("{} {}", &pin[..3], &pin[3..])
    } else {
        pin.to_string()
    }
}

/// Circular monogram from the first character of the device name.
fn monogram(name: &str, size: f32, colors: &gpui_component::ThemeColor) -> Div {
    let ch = name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    div()
        .w(px(size))
        .h(px(size))
        .rounded_full()
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(colors.accent.opacity(0.18))
        .text_color(colors.accent)
        .text_size(px(size * 0.42))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(ch)
}

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
        let vnc_username_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("settings.vnc_username_placeholder").to_string())
        });
        vnc_username_input.update(cx, |s, cx| {
            s.set_value(settings.vnc_username.clone(), window, cx);
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
        let sub_activation = cx.observe_window_activation(window, |_this, _w, cx| cx.notify());
        // Follow system appearance: when the theme setting is System, switch with the OS
        // light/dark mode.
        let sub_appearance = cx.observe_window_appearance(window, |this, window, cx| {
            if this.engine.settings().theme == ThemePref::System {
                apply_theme_pref(ThemePref::System, window, cx);
            }
            cx.notify();
        });

        Self {
            my_fp_short: engine.fingerprint_short(),
            trusted: engine.trusted_short_fps(),
            update_status: engine.update_status(),
            engine,
            _subscriptions: vec![sub_activation, sub_appearance],
            devices: BTreeMap::new(),
            selected: None,
            status: t!("status.ready").to_string(),
            status_tone: StatusTone::Info,
            host_on: false,
            daemon_online: false,
            daemon_perms: None,
            connecting: None,
            admission: None,
            pin_dialog: None,
            pin_input: cx.new(|cx| {
                InputState::new(window, cx).placeholder(t!("home.pin_placeholder").to_string())
            }),
            search_input: cx.new(|cx| {
                InputState::new(window, cx).placeholder(t!("home.search_placeholder").to_string())
            }),
            manual_input: cx.new(|cx| {
                InputState::new(window, cx).placeholder(t!("home.manual_placeholder").to_string())
            }),
            show_manual: false,
            manual_error: None,
            settings_open: false,
            dialog_seq: 0,
            device_name_input,
            vnc_username_input,
            vnc_password_input,
            focus,
            bridge_alive,
        }
    }
}

impl Drop for HomeView {
    fn drop(&mut self) {
        self.bridge_alive.store(false, Ordering::Relaxed);
    }
}

impl HomeView {
    fn set_status(&mut self, text: impl Into<String>, tone: StatusTone) {
        self.status = text.into();
        self.status_tone = tone;
    }

    // ---- events and actions ----

    fn toggle_host(&mut self, cx: &mut Context<Self>) {
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

    fn connect_device(&mut self, fp: &str, cx: &mut Context<Self>) {
        if self.connecting.is_some() {
            self.set_status(t!("status.connecting_other").to_string(), StatusTone::Warn);
            cx.notify();
            return;
        }
        let Some(row) = self.devices.get(fp).cloned() else {
            return;
        };
        self.start_connect(row.name.clone(), row.addr, cx);
    }

    fn start_connect(&mut self, name: String, addr: SocketAddr, cx: &mut Context<Self>) {
        match self.engine.connect_to(addr) {
            Ok(()) => {
                self.connecting = Some(name.clone());
                self.set_status(t!("status.connecting", name = name), StatusTone::Info);
            }
            Err(e) => self.set_status(
                t!("status.connect_failed", err = format!("{e:#}")),
                StatusTone::Err,
            ),
        }
        cx.notify();
    }

    fn connect_manual(&mut self, cx: &mut Context<Self>) {
        let raw = self.manual_input.read(cx).value().trim().to_string();
        if raw.is_empty() {
            return;
        }
        // First try parsing a full SocketAddr (covers 1.2.3.4:7890, [::1]:7890);
        // on failure treat it as a bare address: append the default port to IPv4,
        // or wrap bare IPv6 in brackets first.
        let addr = raw.parse::<SocketAddr>().ok().or_else(|| {
            if raw.parse::<std::net::Ipv4Addr>().is_ok() {
                format!("{raw}:{}", removent_proto::DEFAULT_PORT)
                    .parse()
                    .ok()
            } else if raw.parse::<std::net::Ipv6Addr>().is_ok() {
                format!("[{raw}]:{}", removent_proto::DEFAULT_PORT)
                    .parse()
                    .ok()
            } else {
                None
            }
        });
        match addr {
            Some(addr) => {
                self.manual_error = None;
                self.start_connect(raw.clone(), addr, cx);
            }
            None => {
                self.manual_error = Some(t!("manual.invalid_addr").to_string());
                cx.notify();
            }
        }
    }

    fn answer_admission(&mut self, allow: bool, cx: &mut Context<Self>) {
        if let Some(a) = self.admission.take() {
            self.engine.answer_admission(a.request_id, allow);
            self.set_status(
                if allow {
                    t!("status.admission_allowed", peer = a.peer_name)
                } else {
                    t!("status.admission_denied", peer = a.peer_name)
                },
                if allow {
                    StatusTone::Ok
                } else {
                    StatusTone::Warn
                },
            );
        }
        cx.notify();
    }

    /// Clear the PIN input: every path that closes the Entry dialog must leave the field
    /// empty so the next Entry dialog does not show the previous 6 digits.
    fn clear_pin_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pin_input
            .update(cx, |s, cx| s.set_value("", window, cx));
    }

    fn submit_pin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(PinDialog::Entry(tx)) = self.pin_dialog.take() {
            let pin = self.pin_input.read(cx).value().to_string();
            let _ = tx.send(pin);
            self.clear_pin_input(window, cx);
            self.set_status(t!("status.pin_submitted").to_string(), StatusTone::Info);
        }
        cx.notify();
    }

    fn cancel_pin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(PinDialog::Entry(tx)) = self.pin_dialog.take() {
            let _ = tx.send(String::new());
            self.clear_pin_input(window, cx);
            self.set_status(t!("status.pairing_cancelled").to_string(), StatusTone::Warn);
        }
        cx.notify();
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        let name = self.device_name_input.read(cx).value().trim().to_string();
        let vnc_username = self.vnc_username_input.read(cx).value().trim().to_string();
        let vnc_password = self.vnc_password_input.read(cx).value().to_string();
        if name.is_empty() {
            self.set_status(t!("status.device_name_empty").to_string(), StatusTone::Warn);
            cx.notify();
            return;
        }
        match self.engine.update_settings(|s| {
            s.device_name = name.clone();
            s.vnc_username = vnc_username.clone();
            s.vnc_password = vnc_password.clone();
        }) {
            Ok(()) => {
                self.trusted = self.engine.trusted_short_fps();
                self.my_fp_short = self.engine.fingerprint_short();
                self.set_status(t!("status.settings_saved").to_string(), StatusTone::Ok);
            }
            Err(e) => self.set_status(t!("status.settings_save_failed", err = e), StatusTone::Err),
        }
        cx.notify();
    }

    fn set_theme(&mut self, pref: ThemePref, window: &mut Window, cx: &mut Context<Self>) {
        // A save failure is non-fatal here: the in-memory value already applies.
        let _ = self.engine.update_settings(|s| s.theme = pref);
        apply_theme_pref(pref, window, cx);
        cx.notify();
    }

    fn set_language(&mut self, lang: Language, cx: &mut Context<Self>) {
        let _ = self.engine.update_settings(|s| s.language = lang);
        rust_i18n::set_locale(removent_core::resolve_locale(lang));
        // Status strings are cached translations: reset them, otherwise they keep the
        // previous language until the next status change.
        self.status = t!("status.ready").to_string();
        self.status_tone = StatusTone::Info;
        self.manual_error = None;
        cx.notify();
    }

    fn handle_event(&mut self, ev: UiEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            UiEvent::DeviceFound { fp, name, addr } => {
                self.devices.insert(fp, DeviceRow { name, addr });
            }
            UiEvent::DeviceLost(fp) => {
                self.devices.remove(&fp);
                if self.selected.as_deref() == Some(&fp) {
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
            UiEvent::ClientNeedsPin(tx) => {
                self.dialog_seq += 1;
                self.pin_dialog = Some(PinDialog::Entry(tx));
                // Start from an empty field and hand it the keyboard focus.
                self.clear_pin_input(window, cx);
                let focus = self.pin_input.focus_handle(cx);
                window.focus(&focus);
            }
            UiEvent::SessionReady { codec } => {
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
                        .frames
                        .take_rx()
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
            UiEvent::ConnectFailed(e) => {
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
            UiEvent::SessionClosed(r) => {
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

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = cx.theme().is_dark();
        let colors = cx.theme().colors;
        TitleBar::new()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(icon_16("monitor").text_color(colors.accent))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Removent"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    // Right-side padding so the settings button does not hug the window edge.
                    .pr_3()
                    .child(
                        Button::new("toggle-theme")
                            .icon(icon_16(if dark { "sun" } else { "moon" }))
                            .ghost()
                            .tooltip(if dark {
                                t!("titlebar.to_light").to_string()
                            } else {
                                t!("titlebar.to_dark").to_string()
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                let next = if cx.theme().is_dark() {
                                    ThemePref::Light
                                } else {
                                    ThemePref::Dark
                                };
                                this.set_theme(next, window, cx);
                            })),
                    )
                    .child(
                        Button::new("open-settings")
                            .icon(icon_16("settings"))
                            .ghost()
                            .tooltip(t!("settings.title").to_string())
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.settings_open = !this.settings_open;
                                cx.notify();
                            })),
                    ),
            )
    }

    fn render_device_row(
        &self,
        fp: &str,
        row: &DeviceRow,
        trusted: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let selected = self.selected.as_deref() == Some(fp);
        let fp_sel = fp.to_string();
        let fp_dbl = fp.to_string();
        let mut el = div()
            .id(gpui::ElementId::Name(format!("dev-{fp}").into()))
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .rounded(px(8.))
            .cursor_pointer()
            // Single-click selects; double-click connects directly.
            .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _w, cx| {
                if ev.click_count() >= 2 {
                    this.connect_device(&fp_dbl, cx);
                } else {
                    this.selected = Some(fp_sel.clone());
                    this.settings_open = false;
                    cx.notify();
                }
            }))
            .child(monogram(&row.name, 30., &colors))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(row.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.muted_foreground)
                            .child(fmt_addr(&row.addr)),
                    ),
            )
            .when(trusted, |el| {
                el.child(
                    Tag::success()
                        .rounded_full()
                        .small()
                        .child(t!("device.paired").to_string()),
                )
            });
        if selected {
            el = el.bg(colors.list_active);
        } else {
            el = el.hover(|s| s.bg(colors.list_hover));
        }
        el
    }

    fn render_sidebar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let query = self.search_input.read(cx).value().to_lowercase();
        let rows: Vec<(String, DeviceRow, bool)> = self
            .devices
            .iter()
            .filter(|(_, r)| query.is_empty() || r.name.to_lowercase().contains(&query))
            .map(|(fp, r)| (fp.clone(), r.clone(), self.trusted.contains(fp.as_str())))
            .collect();

        let mut list = div()
            .id("device-list")
            .flex()
            .flex_col()
            .flex_1()
            .px_2()
            .pb_2()
            .overflow_y_scroll();
        if rows.is_empty() {
            list = list.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .py_8()
                    .when(self.devices.is_empty(), |el| {
                        el.child(Spinner::new().color(colors.muted_foreground))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(colors.muted_foreground)
                                    .child(t!("device.searching").to_string()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    // gpui's built-in opacity is multiplicative: a muted base
                                    // (≈60%) ×0.7 ≈ 42% alpha.
                                    .text_color(colors.muted_foreground.opacity(0.7))
                                    .child(t!("device.searching_hint").to_string()),
                            )
                    })
                    .when(!self.devices.is_empty(), |el| {
                        el.child(
                            div()
                                .text_size(px(12.))
                                .text_color(colors.muted_foreground)
                                .child(t!("device.no_match").to_string()),
                        )
                    }),
            );
        }
        for (fp, row, is_trusted) in rows {
            list = list.child(self.render_device_row(&fp, &row, is_trusted, cx));
        }

        // Manual connect: collapsed to a + button in the header by default; expands into a
        // bottom input bar.
        let mut manual = div().flex().flex_col().gap_2();
        if self.show_manual {
            manual = manual.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .border_t_1()
                    .border_color(colors.border)
                    .child(Input::new(&self.manual_input).prefix(icon_16("wifi")))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("connect-manual")
                                    .label(t!("action.connect").to_string())
                                    .primary()
                                    .flex_1()
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.connect_manual(cx);
                                    })),
                            )
                            .child(
                                Button::new("hide-manual")
                                    .label(t!("action.cancel").to_string())
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.show_manual = false;
                                        this.manual_error = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when_some(self.manual_error.clone(), |el, err| {
                        el.child(
                            div()
                                .text_size(px(11.))
                                .text_color(colors.danger)
                                .child(err),
                        )
                    }),
            );
        }

        let _ = window;
        div()
            .w(px(300.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(colors.border)
            .bg(colors.sidebar)
            .child(
                // List header: title + count + manual-connect entry
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .pt_3()
                    .pb_1()
                    .child(section_label(
                        t!("device.count", count = self.devices.len()),
                        cx,
                    ))
                    .child(
                        Button::new("show-manual")
                            .icon(icon_16("plus"))
                            .ghost()
                            .compact()
                            .tooltip(t!("device.manual_tooltip").to_string())
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.show_manual = true;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .pb_2()
                    .child(Input::new(&self.search_input).prefix(icon_16("search"))),
            )
            .child(list)
            .child(manual)
    }

    /// Empty state: the "quick start" two-column cards (control other devices / be controlled
    /// by others), including the permission checklist.
    /// When narrow (window width < 1080) the two cards stack vertically.
    fn render_empty_detail(&self, narrow: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let host_on = self.host_on;

        // Left-card hint row: icon + one-line tip.
        let hint = |ic: &'static str, text: String| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(icon_16(ic).text_color(colors.muted_foreground))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.muted_foreground)
                        .child(text),
                )
        };

        // Permission checklist row: green check when granted; orange warning + open
        // System Settings when not.
        let perm_row = |kind: PermissionKind| {
            let granted = kind.granted();
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    icon_16(if granted { "check" } else { "alert-triangle" }).text_color(
                        if granted {
                            colors.success
                        } else {
                            colors.warning
                        },
                    ),
                )
                .child(div().flex_1().text_size(px(12.)).child(format!(
                    "{} · {}",
                    kind.title(),
                    kind.purpose()
                )))
                .when(!granted, |el| {
                    el.child(
                        Button::new(gpui::ElementId::Name(
                            format!("perm-{}", kind.slug()).into(),
                        ))
                        .label(t!("permissions.open_settings").to_string())
                        .outline()
                        .compact()
                        .on_click(move |_, _, _| kind.request_and_open_settings()),
                    )
                })
        };

        // Stale-TCC hint: the app's own preflight says granted but the long-lived
        // daemon reports not-granted (grants do not propagate to an already-running
        // process), so the daemon needs a restart.
        let daemon_stale = self.daemon_online
            && self.daemon_perms.is_some_and(|(sr, ax)| {
                (PermissionKind::ScreenCapture.granted() && !sr)
                    || (PermissionKind::Accessibility.granted() && !ax)
            });

        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .when(!narrow, |el| el.flex_row())
                    .gap_4()
                    .w_full()
                    .when(!narrow, |el| el.max_w(px(720.)))
                    .px_8()
                    .child(
                        // Left card: control other devices
                        grouped_card(cx)
                            .when(!narrow, |el| el.flex_1())
                            .when(narrow, |el| el.w_full())
                            .p_5()
                            .gap_3()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(t!("empty.control_title").to_string()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(colors.muted_foreground)
                                    .child(t!("empty.control_desc").to_string()),
                            )
                            .child(Divider::horizontal())
                            .child(hint("search", t!("empty.hint_search").to_string()))
                            .child(hint("plus", t!("empty.hint_manual").to_string())),
                    )
                    .child(
                        // Right card: be controlled by others
                        grouped_card(cx)
                            .when(!narrow, |el| el.flex_1())
                            .when(narrow, |el| el.w_full())
                            .p_5()
                            .gap_3()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(t!("empty.controlled_title").to_string()),
                            )
                            .child(
                                // Own fingerprint (pairing credential) + copy
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        icon_16("shield-check").text_color(colors.muted_foreground),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_size(px(12.))
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .child(format!("fp:{}", self.my_fp_short)),
                                    )
                                    .child(
                                        Button::new("copy-fp-empty")
                                            .icon(icon_16("copy"))
                                            .ghost()
                                            .compact()
                                            .tooltip(t!("empty.copy_fp_tooltip").to_string())
                                            .on_click(cx.listener(|this, _, _w, cx| {
                                                cx.write_to_clipboard(
                                                    gpui::ClipboardItem::new_string(
                                                        this.my_fp_short.clone(),
                                                    ),
                                                );
                                            })),
                                    ),
                            )
                            .child(
                                // Host service switch (same entry point as the bottom status bar)
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .child(t!("host.service").to_string()),
                                    )
                                    .child(
                                        Switch::new("host-switch-empty").checked(host_on).on_click(
                                            cx.listener(|this, _checked, _w, cx| {
                                                this.toggle_host(cx);
                                            }),
                                        ),
                                    ),
                            )
                            .child(Divider::horizontal())
                            .child(perm_row(PermissionKind::ScreenCapture))
                            .child(perm_row(PermissionKind::Accessibility))
                            .when(daemon_stale, |el| {
                                el.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(icon_16("alert-triangle").text_color(colors.warning))
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(colors.warning)
                                                .child(
                                                    t!("permissions.daemon_restart_hint")
                                                        .to_string(),
                                                ),
                                        ),
                                )
                            }),
                    ),
            )
    }

    fn render_device_detail(
        &self,
        fp: &str,
        row: &DeviceRow,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let connecting = self.connecting.is_some();
        let fp_c = fp.to_string();
        let fp_short = &fp[..8.min(fp.len())];
        let trusted = self.trusted.contains(fp);

        let meta_row = |label: String, value: String| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .px_4()
                .py(px(10.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.muted_foreground)
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(value),
                )
        };

        div()
            .flex()
            .flex_col()
            .gap_6()
            .p_8()
            .child(
                // Device header
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(monogram(&row.name, 48., &colors))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(20.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(row.name.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(dot(colors.success))
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(colors.muted_foreground)
                                            .child(t!("device.online").to_string()),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        // While connecting this becomes "Cancel": after hanging up, the engine
                        // sends SessionClosed to reset `connecting`.
                        Button::new("connect-device")
                            .icon(icon_16(if connecting { "x" } else { "monitor" }))
                            .label(if connecting {
                                t!("action.cancel").to_string()
                            } else {
                                t!("action.connect").to_string()
                            })
                            .when(!connecting, |b| b.primary())
                            .when(connecting, |b| b.danger())
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                if this.connecting.is_some() {
                                    this.engine.disconnect_client();
                                } else {
                                    this.connect_device(&fp_c, cx);
                                }
                            })),
                    )
                    .child(
                        Button::new("connect-files")
                            .icon(icon_16("folder"))
                            .label(t!("action.files").to_string())
                            .outline()
                            .disabled(true)
                            .tooltip(t!("device.files_tooltip").to_string()),
                    ),
            )
            .child(
                // Device info: inset-grouped card, hairline separators, monospace font
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(card_title(t!("device.info").to_string(), cx))
                    .child(
                        grouped_card(cx)
                            .child(meta_row(
                                t!("device.address").to_string(),
                                fmt_addr(&row.addr),
                            ))
                            .child(Divider::horizontal())
                            .child(meta_row(
                                t!("device.fingerprint").to_string(),
                                fp_short.to_string(),
                            ))
                            .child(Divider::horizontal())
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .px_4()
                                    .py(px(10.))
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(colors.muted_foreground)
                                            .child(t!("device.trust").to_string()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(if trusted {
                                                colors.success
                                            } else {
                                                colors.muted_foreground
                                            })
                                            .child(if trusted {
                                                t!("device.paired").to_string()
                                            } else {
                                                t!("device.unpaired").to_string()
                                            }),
                                    ),
                            ),
                    ),
            )
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let settings = self.engine.settings();
        let vnc_enabled = settings.vnc_enabled;
        let admission_idx = match settings.admission {
            AdmissionMode::AlwaysAsk => 0,
            AdmissionMode::TrustedAuto => 1,
            AdmissionMode::DenyAll => 2,
        };
        let theme_idx = match settings.theme {
            ThemePref::System => 0,
            ThemePref::Dark => 1,
            ThemePref::Light => 2,
        };
        let language_idx = match settings.language {
            Language::System => 0,
            Language::En => 1,
            Language::ZhCn => 2,
        };

        div()
            .id("settings")
            .flex()
            .flex_col()
            .gap_5()
            .p_8()
            .w_full()
            .h_full()
            // Small windows (e.g. 480 high) must still let the save button scroll into view.
            .overflow_y_scroll()
            .max_w(px(640.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(t!("settings.title").to_string()),
                    )
                    .child(
                        Button::new("close-settings")
                            .icon(icon_16("x"))
                            .ghost()
                            .tooltip(t!("action.back").to_string())
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.settings_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                // General: device name / appearance / language
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(card_title(t!("settings.general").to_string(), cx))
                    .child(
                        grouped_card(cx)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .px_4()
                                    .py_3()
                                    .child(
                                        div()
                                            .w(px(56.))
                                            .flex_shrink_0()
                                            .text_size(px(13.))
                                            .child(t!("settings.device_name").to_string()),
                                    )
                                    .child(
                                        div().flex_1().child(Input::new(&self.device_name_input)),
                                    ),
                            )
                            .child(Divider::horizontal())
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .px_4()
                                    .py_3()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .child(t!("settings.appearance").to_string()),
                                    )
                                    .child({
                                        let view = cx.entity().clone();
                                        segmented(
                                            "theme",
                                            &[
                                                t!("settings.theme.system").to_string(),
                                                t!("settings.theme.dark").to_string(),
                                                t!("settings.theme.light").to_string(),
                                            ],
                                            theme_idx,
                                            Rc::new(move |i, _ev, window, app| {
                                                let pref = match i {
                                                    1 => ThemePref::Dark,
                                                    2 => ThemePref::Light,
                                                    _ => ThemePref::System,
                                                };
                                                view.update(app, |this, cx| {
                                                    this.set_theme(pref, window, cx)
                                                });
                                            }),
                                            cx,
                                        )
                                    }),
                            )
                            .child(Divider::horizontal())
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .px_4()
                                    .py_3()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .child(t!("settings.language").to_string()),
                                    )
                                    .child({
                                        let view = cx.entity().clone();
                                        segmented(
                                            "language",
                                            &[
                                                t!("settings.language.system").to_string(),
                                                "English".to_string(),
                                                "中文".to_string(),
                                            ],
                                            language_idx,
                                            Rc::new(move |i, _ev, _w, app| {
                                                let lang = match i {
                                                    1 => Language::En,
                                                    2 => Language::ZhCn,
                                                    _ => Language::System,
                                                };
                                                view.update(app, |this, cx| {
                                                    this.set_language(lang, cx)
                                                });
                                            }),
                                            cx,
                                        )
                                    }),
                            ),
                    ),
            )
            .child(
                // Security: admission control
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(card_title(t!("settings.security").to_string(), cx))
                    .child(
                        grouped_card(cx).child(
                            // Label above the selector: at the 720px minimum window width the
                            // settings pane is only ~356px wide, and a side-by-side row would
                            // clip the third segment (especially with the English labels).
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .px_4()
                                .py_3()
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .child(t!("settings.admission").to_string()),
                                )
                                .child({
                                    let view = cx.entity().clone();
                                    segmented(
                                        "admission",
                                        &[
                                            t!("settings.admission.ask").to_string(),
                                            t!("settings.admission.trusted").to_string(),
                                            t!("settings.admission.deny").to_string(),
                                        ],
                                        admission_idx,
                                        Rc::new(move |i, _ev, _w, app| {
                                            let mode = match i {
                                                0 => AdmissionMode::AlwaysAsk,
                                                1 => AdmissionMode::TrustedAuto,
                                                _ => AdmissionMode::DenyAll,
                                            };
                                            view.update(app, |this, cx| {
                                                let _ = this
                                                    .engine
                                                    .update_settings(|s| s.admission = mode);
                                                this.set_status(
                                                    t!("status.admission_mode_updated").to_string(),
                                                    StatusTone::Ok,
                                                );
                                                cx.notify();
                                            });
                                        }),
                                        cx,
                                    )
                                }),
                        ),
                    ),
            )
            .child(
                // Data: data directory
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(card_title(t!("settings.data").to_string(), cx))
                    .child(
                        grouped_card(cx).child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .px_4()
                                .py_3()
                                .child(
                                    div()
                                        .w(px(56.))
                                        .flex_shrink_0()
                                        .text_size(px(13.))
                                        .child(t!("settings.data_dir").to_string()),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(px(12.))
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_color(colors.muted_foreground)
                                        .overflow_hidden()
                                        .child(self.engine.data_dir().display().to_string()),
                                )
                                .child(
                                    Button::new("open-data-dir")
                                        .label(t!("action.open").to_string())
                                        .outline()
                                        .compact()
                                        .on_click(cx.listener(|this, _, _w, _cx| {
                                            let _ = std::process::Command::new("open")
                                                .arg(this.engine.data_dir())
                                                .spawn();
                                        })),
                                ),
                        ),
                    ),
            )
            .child(
                // Compatibility: standard RFB/VNC for Apple Remote Desktop.
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(card_title(t!("settings.vnc").to_string(), cx))
                    .child(
                        grouped_card(cx)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_3()
                                    .px_4()
                                    .py_3()
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .child(t!("settings.vnc_enabled").to_string()),
                                    )
                                    .child(
                                        Switch::new("vnc-enabled").checked(vnc_enabled).on_click(
                                            cx.listener(|this, _checked, _w, cx| {
                                                let result = this.engine.update_settings(|s| {
                                                    s.vnc_enabled = !s.vnc_enabled
                                                });
                                                match result {
                                                    Ok(()) => this.set_status(
                                                        t!("status.vnc_updated").to_string(),
                                                        StatusTone::Ok,
                                                    ),
                                                    Err(e) => this.set_status(
                                                        t!("status.settings_save_failed", err = e),
                                                        StatusTone::Err,
                                                    ),
                                                }
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                            )
                            .child(Divider::horizontal())
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .px_4()
                                    .py_3()
                                    .child(
                                        div()
                                            .w(px(112.))
                                            .flex_shrink_0()
                                            .text_size(px(13.))
                                            .child(t!("settings.vnc_username").to_string()),
                                    )
                                    .child(
                                        div().flex_1().child(Input::new(&self.vnc_username_input)),
                                    ),
                            )
                            .child(Divider::horizontal())
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .px_4()
                                    .py_3()
                                    .child(
                                        div()
                                            .w(px(112.))
                                            .flex_shrink_0()
                                            .text_size(px(13.))
                                            .child(t!("settings.vnc_password").to_string()),
                                    )
                                    .child(
                                        div().flex_1().child(
                                            Input::new(&self.vnc_password_input).mask_toggle(),
                                        ),
                                    ),
                            )
                            .child(Divider::horizontal())
                            .child(
                                div()
                                    .px_4()
                                    .py_3()
                                    .text_size(px(12.))
                                    .text_color(colors.muted_foreground)
                                    .child(t!("settings.vnc_hint").to_string()),
                            ),
                    ),
            )
            .child(self.render_update_section(cx))
            .child(
                div().flex().justify_end().gap_2().child(
                    Button::new("save-settings")
                        .label(t!("action.save").to_string())
                        .primary()
                        .on_click(cx.listener(|this, _, _w, cx| this.save_settings(cx))),
                ),
            )
    }

    /// Software update (release.md §3): current version, check button, auto-check
    /// toggle, and the state-machine line (available/downloading/ready/failed).
    fn render_update_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let auto_check = self.engine.settings().update_check_enabled;
        let busy = self.update_status.is_busy();

        // One status line per state, colored by severity; spinners while working.
        let status_line = |text: String, color: gpui::Hsla| {
            div().text_size(px(12.)).text_color(color).child(text)
        };
        let spinner_line = |text: String| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(Spinner::new().color(colors.muted_foreground))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.muted_foreground)
                        .child(text),
                )
        };

        let mut state_block = div().flex().flex_col().gap_2().px_4().pb_3();
        match &self.update_status {
            UpdateStatus::Idle => {}
            UpdateStatus::Checking => {
                state_block = state_block.child(spinner_line(t!("update.checking").to_string()));
            }
            UpdateStatus::UpToDate => {
                state_block = state_block.child(status_line(
                    t!("update.up_to_date").to_string(),
                    colors.success,
                ));
            }
            UpdateStatus::Available { version, notes } => {
                state_block = state_block
                    .child(status_line(
                        t!("update.available", version = version).to_string(),
                        colors.accent,
                    ))
                    .when(!notes.is_empty(), |el| {
                        el.child(
                            div()
                                .text_size(px(11.))
                                .text_color(colors.muted_foreground)
                                .child(notes.clone()),
                        )
                    })
                    .child(
                        div().flex().justify_end().child(
                            Button::new("update-download")
                                .label(t!("update.download_install").to_string())
                                .primary()
                                .compact()
                                .on_click(cx.listener(|this, _, _w, cx| {
                                    this.engine.download_update();
                                    cx.notify();
                                })),
                        ),
                    );
            }
            UpdateStatus::Downloading { version } => {
                state_block = state_block.child(spinner_line(
                    t!("update.downloading", version = version).to_string(),
                ));
            }
            UpdateStatus::Verifying { .. } => {
                state_block = state_block.child(spinner_line(t!("update.verifying").to_string()));
            }
            UpdateStatus::ReadyToInstall { version } => {
                let session_active = self.engine.session_active();
                state_block = state_block.child(status_line(
                    t!("update.ready", version = version).to_string(),
                    colors.accent,
                ));
                if session_active {
                    // Deferred: installing would kill the live session.
                    state_block = state_block.child(status_line(
                        t!("update.session_active_hint").to_string(),
                        colors.warning,
                    ));
                }
                state_block = state_block.child(
                    div().flex().justify_end().child(
                        Button::new("update-install")
                            .label(t!("update.install_now").to_string())
                            .primary()
                            .compact()
                            .on_click(cx.listener(|this, _, _w, cx| {
                                // The engine re-checks for live sessions; surface a refusal.
                                if let Err(reason) = this.engine.install_update() {
                                    this.update_status = UpdateStatus::Failed(reason);
                                }
                                cx.notify();
                            })),
                    ),
                );
            }
            UpdateStatus::Swapping { .. } | UpdateStatus::Relaunching => {
                state_block = state_block.child(spinner_line(t!("update.installing").to_string()));
            }
            UpdateStatus::Failed(reason) => {
                state_block = state_block.child(status_line(reason.clone(), colors.danger));
            }
        }

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(card_title(t!("update.section").to_string(), cx))
            .child(
                grouped_card(cx)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_4()
                            .py_3()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .child(t!("update.current").to_string()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(12.))
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_color(colors.muted_foreground)
                                    .child(env!("CARGO_PKG_VERSION")),
                            )
                            .child(
                                Button::new("update-check")
                                    .label(t!("update.check_now").to_string())
                                    .outline()
                                    .compact()
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.engine.check_for_updates();
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(Divider::horizontal())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .px_4()
                            .py_3()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .child(t!("update.auto_check").to_string()),
                            )
                            .child(
                                Switch::new("update-auto-check")
                                    .checked(auto_check)
                                    .on_click(cx.listener(|this, _checked, _w, cx| {
                                        let on = !this.engine.settings().update_check_enabled;
                                        let _ = this
                                            .engine
                                            .update_settings(|s| s.update_check_enabled = on);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(state_block),
            )
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let tone_color = match self.status_tone {
            StatusTone::Info => colors.muted_foreground,
            StatusTone::Ok => colors.success,
            StatusTone::Warn => colors.warning,
            StatusTone::Err => colors.danger,
        };
        let host_on = self.host_on;
        div()
            .h(px(34.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .px_3()
            .border_t_1()
            .border_color(colors.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(dot(tone_color))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(tone_color)
                            .child(self.status.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.muted_foreground)
                            .child(if !self.daemon_online {
                                t!("status.daemon_offline_short").to_string()
                            } else if host_on {
                                t!("status.host_running").to_string()
                            } else {
                                t!("status.host_stopped").to_string()
                            }),
                    )
                    .child(
                        Switch::new("host-switch")
                            .checked(host_on)
                            .on_click(cx.listener(|this, _checked, _w, cx| {
                                this.toggle_host(cx);
                            })),
                    ),
            )
    }

    fn render_pin_dialog(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let dialog = self.pin_dialog.as_ref()?;
        let colors = cx.theme().colors;
        let card = div()
            .w(px(360.))
            .p_6()
            .rounded(px(14.))
            .bg(colors.popover)
            .border_1()
            .border_color(colors.border)
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_4();
        let card = match dialog {
            PinDialog::Display(pin) => card
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(t!("pairing.request_title").to_string()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.muted_foreground)
                        .child(t!("pairing.display_desc").to_string()),
                )
                .child(
                    div().flex().justify_center().py_2().child(
                        div()
                            .text_size(px(28.))
                            .font_family(cx.theme().mono_font_family.clone())
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(colors.accent)
                            .child(group_pin(pin)),
                    ),
                )
                .child(
                    div().flex().justify_end().child(
                        Button::new("dismiss-pin")
                            .label(t!("action.close").to_string())
                            .outline()
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.pin_dialog = None;
                                cx.notify();
                            })),
                    ),
                ),
            PinDialog::Entry(_) => card
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(t!("pairing.entry_title").to_string()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.muted_foreground)
                        .child(t!("pairing.entry_desc").to_string()),
                )
                .child(Input::new(&self.pin_input))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("cancel-pin")
                                .label(t!("action.cancel").to_string())
                                .ghost()
                                .on_click(cx.listener(|this, _, w, cx| this.cancel_pin(w, cx))),
                        )
                        .child(
                            Button::new("submit-pin")
                                .label(t!("action.pair").to_string())
                                .primary()
                                .on_click(cx.listener(|this, _, w, cx| this.submit_pin(w, cx))),
                        ),
                ),
        };
        Some(modal_overlay("pin-overlay", self.dialog_seq, card, cx))
    }

    fn render_admission_dialog(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let a = self.admission.as_ref()?;
        let colors = cx.theme().colors;
        let card = div()
            .w(px(400.))
            .p_6()
            .rounded(px(14.))
            .bg(colors.popover)
            .border_1()
            .border_color(colors.border)
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(t!("admission.title").to_string()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(monogram(&a.peer_name, 36., &colors))
                    .child(
                        div()
                            .child(div().text_size(px(13.)).child(a.peer_name.clone()))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_color(colors.muted_foreground)
                                    .child(format!("fp:{}", a.peer_fp_short)),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(colors.muted_foreground)
                    .child(t!("admission.desc").to_string()),
            )
            .child(
                // Live countdown bar: drains over the daemon's 30s admission window
                // (mirrors removent-daemon DEFAULT_ADMISSION_TIMEOUT, protocol §4.4).
                div().h(px(3.)).rounded_full().bg(colors.border).child(
                    div()
                        .h_full()
                        .rounded_full()
                        .bg(colors.accent.opacity(0.6))
                        .with_animation(
                            ElementId::NamedInteger("admission-countdown".into(), a.request_id),
                            Animation::new(Duration::from_secs(30)),
                            |this, delta| this.w(relative(1. - delta)),
                        ),
                ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("deny")
                            .label(t!("action.deny").to_string())
                            .danger()
                            .on_click(
                                cx.listener(|this, _, _w, cx| this.answer_admission(false, cx)),
                            ),
                    )
                    .child(
                        Button::new("allow")
                            .label(t!("action.allow").to_string())
                            .primary()
                            .on_click(
                                cx.listener(|this, _, _w, cx| this.answer_admission(true, cx)),
                            ),
                    ),
            );
        Some(modal_overlay("admission-overlay", a.request_id, card, cx))
    }
}

/// Centered modal overlay (scrim background + click interception, preventing clicks from
/// falling through to controls beneath the dialog).
///
/// Entry transition: the scrim fades in while the card slides down into place. `seq` must
/// change on every open — animation state is keyed by element id and may survive a remount,
/// so a fixed id would skip the transition when a dialog is reopened.
fn modal_overlay(id: &'static str, seq: u64, card: Div, cx: &App) -> impl IntoElement {
    let fade = motion::modal_enter();
    let slide = fade.clone();
    div()
        .id(ElementId::Name(id.into()))
        .absolute()
        .inset_0()
        .bg(theme::scrim(cx.theme().is_dark()))
        .flex()
        .items_center()
        .justify_center()
        // Empty handlers for input interception only: prevents clicks and scrolls from
        // falling through to the layer below while a dialog is open (double-clicking a
        // device row, flipping a switch, scrolling the settings page).
        .on_mouse_down(gpui::MouseButton::Left, |_, _, _| {})
        .on_mouse_down(gpui::MouseButton::Right, |_, _, _| {})
        .on_mouse_down(gpui::MouseButton::Middle, |_, _, _| {})
        .on_scroll_wheel(|_, _, _| {})
        .child(card.with_animation(
            ElementId::NamedInteger(format!("{id}-slide").into(), seq),
            slide,
            |this, delta| this.mt(px(-24.) + delta * px(24.)),
        ))
        .with_animation(
            ElementId::NamedInteger(format!("{id}-fade").into(), seq),
            fade,
            |this, delta| this.opacity(delta),
        )
}

/// Apply the theme per settings (including the token overlay).
pub fn apply_theme_pref(pref: ThemePref, window: &mut Window, cx: &mut gpui::App) {
    use gpui_component::theme::{Theme, ThemeMode};
    let dark = match pref {
        ThemePref::Dark => true,
        ThemePref::Light => false,
        ThemePref::System => matches!(
            window.appearance(),
            gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
        ),
    };
    Theme::change(
        if dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
        Some(window),
        cx,
    );
    theme::apply(dark, cx);
}

impl Render for HomeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        // Window width < 1080 stacks the empty-state cards vertically.
        let narrow = window.viewport_size().width < px(1080.);
        let detail = if self.settings_open {
            self.render_settings(cx).into_any_element()
        } else if let Some(fp) = self.selected.clone() {
            match self.devices.get(&fp).cloned() {
                Some(row) => self.render_device_detail(&fp, &row, cx).into_any_element(),
                None => self.render_empty_detail(narrow, cx).into_any_element(),
            }
        } else {
            self.render_empty_detail(narrow, cx).into_any_element()
        };

        div()
            .key_context("Home")
            .track_focus(&self.focus)
            // Esc closes the topmost dialog first, matching the render order
            // below (admission renders after the PIN dialog, hence sits on
            // top): admission = deny (the safe default for a connection
            // request), Entry PIN = cancel pairing, Display PIN = dismiss.
            .on_action(cx.listener(|this, _: &HomeEscape, window, cx| {
                if this.admission.is_some() {
                    this.answer_admission(false, cx);
                } else if matches!(this.pin_dialog, Some(PinDialog::Entry(_))) {
                    this.cancel_pin(window, cx);
                } else if matches!(this.pin_dialog, Some(PinDialog::Display(_))) {
                    this.pin_dialog = None;
                    cx.notify();
                }
            }))
            .flex()
            .flex_col()
            .size_full()
            .bg(colors.background)
            .text_color(colors.foreground)
            .child(self.render_title_bar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.render_sidebar(window, cx))
                    .child(div().flex_1().overflow_hidden().child(detail)),
            )
            .child(self.render_status_bar(cx))
            .children(self.render_pin_dialog(cx))
            .children(self.render_admission_dialog(cx))
    }
}

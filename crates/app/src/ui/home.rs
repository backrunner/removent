//! Home main window: device list + detail panel + pairing/admission dialogs + settings
//! (ui-design §4.1/§4.3/§4.4).
//!
//! Interaction flow: browse/search → single-click to select and view details
//! (double-click connects directly) → session window.
//! Layered surfaces, section navigation, and consistent responsive form controls.

use crate::engine::{Engine, UiEvent};
use crate::permissions::PermissionKind;
use crate::theme;
use crate::ui::connection::{ConnectionDialog, ConnectionDialogEvent};
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
    divider::Divider,
    input::{InputEvent, InputState},
    spinner::Spinner,
    switch::Switch,
};
use removent_client::connection::{ConnectionProtocol, ConnectionStage};
use removent_client::saved::SavedConnection;
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

gpui::actions!(home, [HomeEscape, HomeSettings, HomeConnect, HomeSearch]);

// Search, list/empty state, section header, and local device share one gutter.
const SIDEBAR_INSET: f32 = 16.;

#[derive(Clone)]
struct DeviceRow {
    name: String,
    addr: SocketAddr,
}

/// Sidebar selection: a discovered device, a saved bookmark, or None (this Mac).
#[derive(Clone, PartialEq)]
enum Selection {
    Device(String),
    Saved(String),
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
    /// Saved connection bookmarks (connections.json), refreshed on save/remove.
    saved: Vec<SavedConnection>,
    selected: Option<Selection>,
    status: String,
    status_tone: StatusTone,
    host_on: bool,
    daemon_online: bool,
    /// daemon-reported TCC permissions (screen_recording, accessibility); None until
    /// the first StatusReport arrives.
    daemon_perms: Option<(bool, bool)>,
    local_perms: (bool, bool),
    /// Controlling in progress: waiting for SessionReady to open the viewer (stores the peer name).
    connecting: Option<String>,
    connection_stage: ConnectionStage,
    cancelled_generation: Option<usize>,
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
    connection_dialog: Option<Entity<ConnectionDialog>>,
    connection_subscription: Option<gpui::Subscription>,
    settings_open: bool,
    settings_section: usize,
    device_name_input: Entity<InputState>,
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
    if pin.len() == 6 && pin.is_ascii() {
        format!("{} {}", &pin[..3], &pin[3..])
    } else {
        pin.to_string()
    }
}

/// Consistent device glyph; names carry identity instead of decorative avatars.
fn icon_tile(name: &'static str, size: f32, colors: &gpui_component::ThemeColor) -> Div {
    div()
        .w(px(size))
        .h(px(size))
        .rounded(px(12.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(colors.accent.opacity(0.12))
        .border_1()
        .border_color(colors.accent.opacity(0.18))
        .child(icon(name).size(px(size * 0.5)).text_color(colors.accent))
}

fn device_glyph(size: f32, colors: &gpui_component::ThemeColor) -> Div {
    icon_tile("monitor", size, colors)
}

/// Small muted group heading inside the sidebar list (Saved / Nearby).
fn list_group_label(text: String, cx: &App) -> Div {
    div()
        .px_3()
        .pt_3()
        .pb_1()
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

/// Bookmark glyph keyed by protocol, matching the connection dialog picker.
fn saved_glyph(
    protocol: ConnectionProtocol,
    size: f32,
    colors: &gpui_component::ThemeColor,
) -> Div {
    let name = match protocol {
        ConnectionProtocol::Removent => "wifi",
        ConnectionProtocol::Vnc => "monitor",
        ConnectionProtocol::Rdp => "copy",
    };
    icon_tile(name, size, colors)
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
            saved: engine.saved_connections(),
            engine,
            _subscriptions: subscriptions,
            devices: BTreeMap::new(),
            selected: None,
            status: t!("status.ready").to_string(),
            status_tone: StatusTone::Info,
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
    fn connect_saved(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.connecting.is_some() {
            self.set_status(t!("status.connecting_other").to_string(), StatusTone::Warn);
            cx.notify();
            return;
        }
        let Some(entry) = self.saved.iter().find(|s| s.id == id).cloned() else {
            return;
        };
        let mut request = entry.to_request();
        if entry.needs_credentials() {
            match self.engine.saved_password(&entry.id) {
                Some(password) => request.password = password,
                None => {
                    self.open_connection_dialog(Some(&entry), window, cx);
                    return;
                }
            }
        }
        let name = entry.display_name();
        self.engine.touch_saved_connection(&entry);
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

    fn remove_saved(&mut self, id: &str, cx: &mut Context<Self>) {
        match self.engine.remove_saved_connection(id) {
            Ok(()) => {
                self.saved = self.engine.saved_connections();
                if matches!(&self.selected, Some(Selection::Saved(s)) if s == id) {
                    self.selected = None;
                }
                self.set_status(
                    t!("status.connection_removed").to_string(),
                    StatusTone::Info,
                );
            }
            Err(e) => self.set_status(
                t!("status.connection_save_failed", err = e.to_string()),
                StatusTone::Err,
            ),
        }
        cx.notify();
    }

    fn cancel_connection(&mut self, cx: &mut Context<Self>) {
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

    fn set_connection_stage(&mut self, stage: ConnectionStage, cx: &mut Context<Self>) {
        if self.connecting.is_some() {
            self.connection_stage = stage;
            if let Some(dialog) = &self.connection_dialog {
                dialog.update(cx, |form, cx| form.set_stage(stage, cx));
            }
        }
    }

    fn open_connection_dialog(
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
        if let Some(saved) = prefill {
            let password = self.engine.saved_password(&saved.id);
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
                    ConnectionDialogEvent::Submit => {
                        if this.connecting.is_some() {
                            return;
                        }
                        let Ok(request) = dialog.read(cx).request(cx) else {
                            return;
                        };
                        let memo = dialog.read(cx).memo_name(cx);
                        // A submitted form is a saved bookmark (passwords are never
                        // persisted); a failed save must not block connecting.
                        match this.engine.save_connection(&request, memo.clone()) {
                            Ok(()) => this.saved = this.engine.saved_connections(),
                            Err(e) => this.set_status(
                                t!("status.connection_save_failed", err = e.to_string()),
                                StatusTone::Warn,
                            ),
                        }
                        let name = if memo.is_empty() {
                            format!("{} ({})", request.address, request.protocol.label())
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
                                dialog.update(cx, |form, cx| form.set_connecting(true, None, cx));
                                window.focus(&dialog.focus_handle(cx));
                            }
                            Err(error) => dialog.update(cx, |form, cx| {
                                form.set_connecting(
                                    false,
                                    Some(
                                        t!("status.connect_failed", err = format!("{error:#}"))
                                            .to_string(),
                                    ),
                                    cx,
                                )
                            }),
                        }
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
        let pin = self.pin_input.read(cx).value().trim().to_string();
        if pin.len() != 6 || !pin.bytes().all(|b| b.is_ascii_digit()) {
            return;
        }
        if !matches!(self.pin_dialog, Some(PinDialog::Entry(_))) {
            return;
        }
        if let Some(PinDialog::Entry(tx)) = self.pin_dialog.take() {
            let _ = tx.send(pin);
            self.set_connection_stage(ConnectionStage::Authenticating, cx);
            self.clear_pin_input(window, cx);
            self.set_status(t!("status.pin_submitted").to_string(), StatusTone::Info);
        }
        cx.notify();
    }

    fn cancel_pin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(PinDialog::Entry(tx)) = self.pin_dialog.take() {
            drop(tx);
            self.clear_pin_input(window, cx);
            self.cancel_connection(cx);
        }
        cx.notify();
    }

    fn persist_settings(&mut self, f: impl FnOnce(&mut removent_core::Settings)) -> bool {
        match self.engine.update_settings(f) {
            Ok(()) => true,
            Err(e) => {
                self.set_status(t!("status.settings_save_failed", err = e), StatusTone::Err);
                false
            }
        }
    }

    fn save_device_name(&mut self, cx: &mut Context<Self>) {
        let name = self.device_name_input.read(cx).value().trim().to_string();
        if name.is_empty() {
            self.set_status(t!("status.device_name_empty").to_string(), StatusTone::Warn);
            cx.notify();
            return;
        }
        if self.persist_settings(|s| s.device_name = name) {
            self.set_status(t!("status.settings_saved").to_string(), StatusTone::Ok);
        }
        cx.notify();
    }

    fn save_vnc(&mut self, cx: &mut Context<Self>) {
        let password = self.vnc_password_input.read(cx).value().to_string();
        if self.persist_settings(|s| {
            s.vnc_password = password;
        }) {
            self.set_status(t!("status.vnc_updated").to_string(), StatusTone::Ok);
        }
        cx.notify();
    }

    fn set_theme(&mut self, pref: ThemePref, window: &mut Window, cx: &mut Context<Self>) {
        if self.persist_settings(|s| s.theme = pref) {
            apply_theme_pref(pref, window, cx);
        }
        cx.notify();
    }

    fn set_language(&mut self, lang: Language, window: &mut Window, cx: &mut Context<Self>) {
        if !self.persist_settings(|s| s.language = lang) {
            cx.notify();
            return;
        }
        rust_i18n::set_locale(removent_core::resolve_locale(lang));
        for (input, key) in [
            (&self.search_input, "home.search_placeholder"),
            (&self.pin_input, "home.pin_placeholder"),
            (&self.device_name_input, "home.name_placeholder"),
            (
                &self.vnc_password_input,
                "settings.vnc_password_placeholder",
            ),
        ] {
            input.update(cx, |input, cx| {
                input.set_placeholder(t!(key).to_string(), window, cx)
            });
        }
        // Status strings are cached translations: reset them, otherwise they keep the
        // previous language until the next status change.
        self.status = t!("status.ready").to_string();
        self.status_tone = StatusTone::Info;
        cx.notify();
    }

    fn handle_event(&mut self, ev: UiEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !ev.belongs_to_client(self.engine.client_generation()) {
            return;
        }
        match ev {
            UiEvent::DeviceFound { fp, name, addr } => {
                self.devices.insert(fp, DeviceRow { name, addr });
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
        TitleBar::new()
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child("Removent"),
            )
            .child(
                div()
                    .pr_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        Button::new("add-connection")
                            .icon(icon_16("plus"))
                            .label(t!("connection.add").to_string())
                            .primary()
                            .small()
                            .tooltip(t!("connection.add").to_string())
                            .disabled(self.connecting.is_some())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_connection_dialog(None, window, cx)
                            })),
                    )
                    .child(
                        Button::new("open-settings")
                            .icon(icon_16("settings"))
                            .ghost()
                            .small()
                            .tooltip(t!("settings.title").to_string())
                            .tab(self.settings_open)
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
        let selected =
            !self.settings_open && matches!(&self.selected, Some(Selection::Device(s)) if s == fp);
        let fp_sel = fp.to_string();
        let fp_dbl = fp.to_string();
        let mut el = div()
            .id(gpui::ElementId::Name(format!("dev-{fp}").into()))
            .tab_index(0)
            .border_1()
            .border_color(colors.border.opacity(0.))
            .focus(|style| style.border_color(colors.ring))
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .rounded(px(12.))
            .cursor_default()
            // Single-click selects; double-click connects directly.
            .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _w, cx| {
                if ev.click_count() >= 2 {
                    this.connect_device(&fp_dbl, cx);
                } else {
                    this.selected = Some(Selection::Device(fp_sel.clone()));
                    this.settings_open = false;
                    cx.notify();
                }
            }))
            .child(device_glyph(32., &colors))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .truncate()
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
                el.child(icon_16("shield-check").text_color(colors.muted_foreground))
            });
        if selected {
            el = el
                .bg(colors.accent.opacity(0.12))
                .border_color(colors.accent.opacity(0.25))
                .shadow_sm()
                .hover(|s| s.bg(colors.accent.opacity(0.20)))
                .active(|s| s.bg(colors.accent.opacity(0.28)));
        } else {
            el = el
                .hover(|s| s.bg(colors.list_hover))
                .active(|s| s.bg(colors.list_active));
        }
        el
    }

    fn render_saved_row(
        &self,
        entry: &SavedConnection,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let selected = !self.settings_open
            && matches!(&self.selected, Some(Selection::Saved(id)) if id == &entry.id);
        let id_sel = entry.id.clone();
        let id_dbl = entry.id.clone();
        let mut el = div()
            .id(gpui::ElementId::Name(format!("saved-{}", entry.id).into()))
            .tab_index(0)
            .border_1()
            .border_color(colors.border.opacity(0.))
            .focus(|style| style.border_color(colors.ring))
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .rounded(px(12.))
            .cursor_default()
            // Same interaction as discovered devices: click selects, double-click connects.
            .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, window, cx| {
                if ev.click_count() >= 2 {
                    this.connect_saved(&id_dbl, window, cx);
                } else {
                    this.selected = Some(Selection::Saved(id_sel.clone()));
                    this.settings_open = false;
                    cx.notify();
                }
            }))
            .child(saved_glyph(entry.protocol, 32., &colors))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .truncate()
                            .child(entry.display_name()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.muted_foreground)
                            .truncate()
                            .child(format!(
                                "{} · {}",
                                entry.protocol.short_label(),
                                entry.address()
                            )),
                    ),
            );
        if selected {
            el = el
                .bg(colors.accent.opacity(0.12))
                .border_color(colors.accent.opacity(0.25))
                .shadow_sm()
                .hover(|s| s.bg(colors.accent.opacity(0.20)))
                .active(|s| s.bg(colors.accent.opacity(0.28)));
        } else {
            el = el
                .hover(|s| s.bg(colors.list_hover))
                .active(|s| s.bg(colors.list_active));
        }
        el
    }

    fn render_sidebar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let query = self.search_input.read(cx).value().trim().to_lowercase();
        let rows: Vec<_> = self
            .devices
            .iter()
            .filter(|(_, row)| {
                query.is_empty()
                    || row.name.to_lowercase().contains(&query)
                    || row.addr.to_string().contains(&query)
            })
            .collect();
        let saved_rows: Vec<_> = self
            .saved
            .iter()
            .filter(|entry| {
                query.is_empty()
                    || entry.name.to_lowercase().contains(&query)
                    || entry.host.to_lowercase().contains(&query)
                    || entry.address().to_string().contains(&query)
            })
            .collect();
        let mut list = div()
            .id("device-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap_1()
            .px(px(SIDEBAR_INSET))
            .overflow_y_scroll();
        // Bookmarks come first: they are the user's own connect targets, while
        // discovered devices below depend on LAN presence.
        if !saved_rows.is_empty() {
            list = list.child(list_group_label(t!("saved.section").to_string(), cx));
            for entry in saved_rows.iter().copied() {
                list = list.child(self.render_saved_row(entry, cx));
            }
            if !rows.is_empty() {
                list = list.child(list_group_label(t!("device.nearby").to_string(), cx));
            }
        }
        if rows.is_empty() && (saved_rows.is_empty() || query.is_empty()) {
            list = list.child(
                div()
                    .p_4()
                    .mt_2()
                    .rounded(px(14.))
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.group_box)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .mb_1()
                            .child(icon("wifi").size(px(24.)).text_color(colors.accent)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(colors.muted_foreground)
                            .child(
                                t!(if query.is_empty() {
                                    "device.searching"
                                } else {
                                    "device.no_match"
                                })
                                .to_string(),
                            ),
                    )
                    .when(query.is_empty(), |el| {
                        el.child(
                            div()
                                .text_size(px(12.))
                                .text_color(colors.muted_foreground)
                                .child(t!("device.searching_hint").to_string()),
                        )
                    }),
            );
        }
        for (fp, row) in rows {
            list =
                list.child(self.render_device_row(fp, row, self.trusted.contains(fp.as_str()), cx));
        }
        div()
            .w(px(if window.viewport_size().width < px(1000.) {
                228.
            } else {
                256.
            }))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(colors.border)
            .bg(colors.sidebar)
            .child(
                div()
                    .px(px(SIDEBAR_INSET))
                    .pt_5()
                    .pb_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(section_label(t!("device.section").to_string(), cx))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.muted_foreground)
                            .px_2()
                            .py_1()
                            .rounded(px(6.))
                            .bg(colors.secondary)
                            .child(self.devices.len().to_string()),
                    ),
            )
            .child(
                div().px(px(SIDEBAR_INSET)).pb_3().child(
                    form_input(&self.search_input)
                        .prefix(icon_16("search"))
                        .cleanable(true),
                ),
            )
            .child(list)
            .child(
                div().px(px(SIDEBAR_INSET)).py_3().child(
                    div()
                        .id("local-device")
                        .tab_index(0)
                        .border_1()
                        .border_color(colors.border.opacity(0.))
                        .focus(|style| style.border_color(colors.ring))
                        .flex()
                        .items_center()
                        .gap_3()
                        .p_3()
                        .rounded(px(12.))
                        .when(self.selected.is_none() && !self.settings_open, |el| {
                            el.bg(colors.accent.opacity(0.12))
                                .border_color(colors.accent.opacity(0.2))
                        })
                        .hover(|el| el.bg(colors.list_hover))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _w, cx| {
                            this.selected = None;
                            this.settings_open = false;
                            cx.notify();
                        }))
                        .child(device_glyph(32., &colors))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .child(t!("device.this_mac").to_string()),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(colors.muted_foreground)
                                        .truncate()
                                        .child(self.engine.device_name()),
                                ),
                        )
                        .child(dot(if self.host_on {
                            colors.success
                        } else {
                            colors.muted_foreground.opacity(0.5)
                        })),
                ),
            )
    }

    /// Connection entry point and local sharing controls share one quiet content surface.
    fn render_empty_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let connecting = self.connecting.is_some();
        let permissions = |kind: PermissionKind| {
            let granted = match kind {
                PermissionKind::ScreenCapture => self.local_perms.0,
                PermissionKind::Accessibility => self.local_perms.1,
            };
            div()
                .flex()
                .items_center()
                .gap_3()
                .min_h(px(36.))
                .child(div().flex_1().text_size(px(12.)).child(kind.title()))
                .when(granted, |el| {
                    el.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(colors.muted_foreground)
                                    .child(t!("permissions.granted").to_string()),
                            )
                            .child(icon_16("check").text_color(colors.muted_foreground)),
                    )
                })
                .when(!granted, |el| {
                    el.child(
                        Button::new(ElementId::Name(format!("perm-{}", kind.slug()).into()))
                            .label(t!("permissions.open_settings").to_string())
                            .tooltip(kind.purpose())
                            .ghost()
                            .small()
                            .on_click(move |_, _, _| kind.request_and_open_settings()),
                    )
                })
        };
        let daemon_stale = self.daemon_online
            && self
                .daemon_perms
                .is_some_and(|(sr, ax)| (self.local_perms.0 && !sr) || (self.local_perms.1 && !ax));
        div()
            .id("quick-start-scroll")
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .child(
                // my_auto centers the guide vertically when it fits; the margins
                // collapse to zero once content overflows, so scrolling still works.
                div()
                    .max_w(px(760.))
                    .w_full()
                    .mx_auto()
                    .my_auto()
                    .flex_shrink_0()
                    .p_6()
                    .flex()
                    .flex_col()
                    .gap_5()
                    .child(
                        form_group(cx)
                            .p_6()
                            .bg(gpui::linear_gradient(
                                135.,
                                gpui::linear_color_stop(colors.secondary_active, 0.),
                                gpui::linear_color_stop(colors.group_box, 1.),
                            ))
                            .gap_5()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(11.))
                                    .text_color(colors.accent)
                                    .child(icon_16("wifi"))
                                    .child(t!("connection.lan_badge").to_string()),
                            )
                            .child(
                                div().flex().flex_col().gap_2().child(
                                    div()
                                        .text_size(px(28.))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(t!("connection.welcome").to_string()),
                                ),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(colors.muted_foreground)
                                    .child(t!("connection.welcome_hint").to_string()),
                            )
                            .child(
                                div().flex().child(
                                    Button::new("open-connection-form")
                                        .h(px(38.))
                                        .icon(icon_16(if connecting { "x" } else { "plus" }))
                                        .label(
                                            t!(if connecting {
                                                "action.cancel"
                                            } else {
                                                "connection.add"
                                            })
                                            .to_string(),
                                        )
                                        .primary()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            if this.connecting.is_some() {
                                                this.cancel_connection(cx);
                                            } else {
                                                this.open_connection_dialog(None, window, cx);
                                            }
                                        })),
                                ),
                            ),
                    )
                    .child(
                        form_group(cx)
                            .p_5()
                            .gap_4()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_4()
                                    .child(
                                        div()
                                            .flex_1()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_size(px(14.))
                                                    .font_weight(gpui::FontWeight::MEDIUM)
                                                    .child(t!("sharing.title").to_string()),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .text_color(colors.muted_foreground)
                                                    .child(t!("sharing.description").to_string()),
                                            ),
                                    )
                                    .child(
                                        Switch::new("host-switch")
                                            .small()
                                            .checked(self.host_on)
                                            .on_click(
                                                cx.listener(|this, _, _w, cx| this.toggle_host(cx)),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(permissions(PermissionKind::ScreenCapture))
                                    .child(permissions(PermissionKind::Accessibility)),
                            )
                            .when(daemon_stale, |el| {
                                el.child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(colors.warning)
                                        .child(t!("permissions.daemon_restart_hint").to_string()),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .border_t_1()
                                    .border_color(colors.border)
                                    .pt_3()
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(colors.muted_foreground)
                                            .child(t!("device.fingerprint").to_string()),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(11.))
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .text_color(colors.muted_foreground)
                                            .truncate()
                                            .child(self.my_fp_short.clone()),
                                    )
                                    .child(
                                        Button::new("copy-fp")
                                            .icon(icon_16("copy"))
                                            .ghost()
                                            .small()
                                            .tooltip(t!("device.copy_fingerprint").to_string())
                                            .on_click(cx.listener(|this, _, _w, cx| {
                                                cx.write_to_clipboard(
                                                    gpui::ClipboardItem::new_string(
                                                        this.my_fp_short.clone(),
                                                    ),
                                                );
                                            })),
                                    ),
                            ),
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
            .id("device-detail")
            .size_full()
            .overflow_y_scroll()
            .p_6()
            .max_w(px(760.))
            .mx_auto()
            .child(
                // Device header
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(device_glyph(40., &colors))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(20.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .truncate()
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
                div().flex().gap_2().child(
                    // Cancel immediately releases the form and invalidates queued events.
                    Button::new("connect-device")
                        .icon(icon_16(if connecting { "x" } else { "monitor" }))
                        .label(if connecting {
                            t!("action.cancel").to_string()
                        } else {
                            t!("action.connect").to_string()
                        })
                        .when(!connecting, |b| b.primary())
                        .when(connecting, |b| b.outline())
                        .on_click(cx.listener(move |this, _, _w, cx| {
                            if this.connecting.is_some() {
                                this.cancel_connection(cx);
                            } else {
                                this.connect_device(&fp_c, cx);
                            }
                        })),
                ),
            )
            .child(
                // Device metadata shares the grouped surfaces used by settings.
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(group_title(t!("device.info").to_string(), cx))
                    .child(
                        form_group(cx)
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

    fn render_saved_detail(
        &self,
        entry: &SavedConnection,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors;
        let connecting = self.connecting.is_some();
        let id_connect = entry.id.clone();
        let id_edit = entry.id.clone();
        let id_remove = entry.id.clone();

        let meta_row = |label: String, value: String| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .px_4()
                .py(px(10.))
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(12.))
                        .text_color(colors.muted_foreground)
                        .child(label),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(value),
                )
        };

        div()
            .flex()
            .flex_col()
            .gap_6()
            .id("saved-detail")
            .size_full()
            .overflow_y_scroll()
            .p_6()
            .max_w(px(760.))
            .mx_auto()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(saved_glyph(entry.protocol, 40., &colors))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(20.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(entry.display_name()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(colors.muted_foreground)
                                    .child(t!("saved.subtitle").to_string()),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("connect-saved")
                            .icon(icon_16(if connecting { "x" } else { "monitor" }))
                            .label(if connecting {
                                t!("action.cancel").to_string()
                            } else {
                                t!("action.connect").to_string()
                            })
                            .when(!connecting, |b| b.primary())
                            .when(connecting, |b| b.outline())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.connecting.is_some() {
                                    this.cancel_connection(cx);
                                } else {
                                    this.connect_saved(&id_connect, window, cx);
                                }
                            })),
                    )
                    .when(!connecting, |el| {
                        el.child(
                            Button::new("edit-saved")
                                .label(t!("action.edit").to_string())
                                .outline()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Some(entry) =
                                        this.saved.iter().find(|s| s.id == id_edit).cloned()
                                    {
                                        this.open_connection_dialog(Some(&entry), window, cx);
                                    }
                                })),
                        )
                        .child(
                            Button::new("remove-saved")
                                .icon(icon_16("trash-2"))
                                .ghost()
                                .tooltip(t!("saved.remove").to_string())
                                .on_click(cx.listener(move |this, _, _w, cx| {
                                    this.remove_saved(&id_remove, cx)
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(group_title(t!("device.info").to_string(), cx))
                    .child(
                        form_group(cx)
                            .child(meta_row(
                                t!("saved.protocol").to_string(),
                                entry.protocol.label().to_string(),
                            ))
                            .child(Divider::horizontal())
                            .child(meta_row(
                                t!("device.address").to_string(),
                                entry.address().to_string(),
                            ))
                            .when(!entry.username.is_empty(), |el| {
                                el.child(Divider::horizontal()).child(meta_row(
                                    t!("connection.username").to_string(),
                                    entry.username.clone(),
                                ))
                            }),
                    ),
            )
            .when(entry.password_hint, |el| {
                el.child(
                    div()
                        .p_4()
                        .rounded(px(12.))
                        .bg(colors.accent.opacity(0.08))
                        .text_size(px(12.))
                        .text_color(colors.muted_foreground)
                        .child(t!("saved.credentials_hint").to_string()),
                )
            })
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let settings = self.engine.settings();
        let mut navigation = div()
            .flex()
            .flex_wrap()
            .gap(px(2.))
            .p(px(3.))
            .w_auto()
            .max_w_full()
            .rounded(px(10.))
            .bg(colors.sidebar)
            .border_1()
            .border_color(colors.border);
        navigation.style().align_self = Some(gpui::AlignSelf::FlexStart);
        for (index, name, key) in [
            (0, "settings", "settings.general"),
            (1, "sun", "settings.appearance"),
            (2, "shield-check", "settings.security"),
            (3, "wifi", "settings.sharing"),
            (4, "info", "settings.system"),
        ] {
            navigation = navigation.child(
                Button::new(("settings-tab", index))
                    .ghost()
                    .icon(icon_16(name))
                    .label(t!(key).to_string())
                    .h(px(32.))
                    .rounded(px(7.))
                    .tab(self.settings_section == index)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings_section = index;
                        cx.notify();
                    })),
            );
        }
        let row = || div().flex().flex_col().gap_3().p_5().w_full().min_w_0();
        let header = |name, title, description| {
            section_header(name, t!(title).to_string(), t!(description).to_string(), cx)
        };
        let mut content = div().flex().flex_col().gap_5().w_full().min_w_0();
        match self.settings_section {
            0 => {
                content = content
                    .child(header(
                        "monitor",
                        "settings.general",
                        "settings.general_description",
                    ))
                    .child(
                        form_group(cx).child(
                            row()
                                .child(setting_label(
                                    t!("settings.device_name").to_string(),
                                    t!("settings.device_name_hint").to_string(),
                                    cx,
                                ))
                                .child(
                                    div()
                                        .flex()
                                        .w_full()
                                        .items_center()
                                        .gap_3()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .child(form_input(&self.device_name_input)),
                                        )
                                        .child(
                                            Button::new("save-device-name")
                                                .label(t!("action.save").to_string())
                                                .primary()
                                                .h(px(36.))
                                                .flex_shrink_0()
                                                .disabled(
                                                    self.device_name_input
                                                        .read(cx)
                                                        .value()
                                                        .trim()
                                                        .is_empty()
                                                        || self
                                                            .device_name_input
                                                            .read(cx)
                                                            .value()
                                                            .trim()
                                                            == settings.device_name,
                                                )
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.save_device_name(cx)
                                                })),
                                        ),
                                ),
                        ),
                    )
                    .child(
                        form_group(cx).child(
                            row()
                                .child(setting_label(
                                    t!("settings.language").to_string(),
                                    t!("settings.language_hint").to_string(),
                                    cx,
                                ))
                                .child({
                                    let view = cx.entity();
                                    segmented(
                                        "language",
                                        &[
                                            t!("settings.language.system").to_string(),
                                            "English".into(),
                                            "中文".into(),
                                        ],
                                        match settings.language {
                                            Language::System => 0,
                                            Language::En => 1,
                                            Language::ZhCn => 2,
                                        },
                                        Rc::new(move |i, _, window, app| {
                                            view.update(app, |this, cx| {
                                                this.set_language(
                                                    match i {
                                                        1 => Language::En,
                                                        2 => Language::ZhCn,
                                                        _ => Language::System,
                                                    },
                                                    window,
                                                    cx,
                                                )
                                            });
                                        }),
                                        cx,
                                    )
                                }),
                        ),
                    );
            }
            1 => {
                content = content
                    .child(header(
                        "sun",
                        "settings.appearance",
                        "settings.appearance_description",
                    ))
                    .child(
                        form_group(cx).child(
                            row()
                                .child(setting_label(
                                    t!("settings.color_mode").to_string(),
                                    t!("settings.theme_hint").to_string(),
                                    cx,
                                ))
                                .child({
                                    let view = cx.entity();
                                    segmented(
                                        "theme",
                                        &[
                                            t!("settings.theme.system").to_string(),
                                            t!("settings.theme.dark").to_string(),
                                            t!("settings.theme.light").to_string(),
                                        ],
                                        match settings.theme {
                                            ThemePref::System => 0,
                                            ThemePref::Dark => 1,
                                            ThemePref::Light => 2,
                                        },
                                        Rc::new(move |i, _, window, app| {
                                            view.update(app, |this, cx| {
                                                this.set_theme(
                                                    match i {
                                                        1 => ThemePref::Dark,
                                                        2 => ThemePref::Light,
                                                        _ => ThemePref::System,
                                                    },
                                                    window,
                                                    cx,
                                                )
                                            });
                                        }),
                                        cx,
                                    )
                                })
                                .child(
                                    div()
                                        .mt_2()
                                        .p_4()
                                        .rounded(px(12.))
                                        .bg(colors.background)
                                        .border_1()
                                        .border_color(colors.border)
                                        .child(section_header(
                                            "monitor",
                                            "Removent".into(),
                                            t!("settings.preview_hint").to_string(),
                                            cx,
                                        ))
                                        .child(
                                            div()
                                                .mt_4()
                                                .flex()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .w(px(72.))
                                                        .h(px(54.))
                                                        .rounded(px(8.))
                                                        .bg(colors.sidebar),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .h(px(54.))
                                                        .rounded(px(8.))
                                                        .bg(colors.group_box)
                                                        .shadow_sm()
                                                        .child(
                                                            div()
                                                                .m_3()
                                                                .w(px(48.))
                                                                .h(px(6.))
                                                                .rounded_full()
                                                                .bg(colors.accent),
                                                        ),
                                                ),
                                        ),
                                ),
                        ),
                    );
            }
            2 => {
                content = content
                    .child(header(
                        "shield-check",
                        "settings.security",
                        "settings.security_description",
                    ))
                    .child(
                        form_group(cx).child(
                            row()
                                .child(setting_label(
                                    t!("settings.admission").to_string(),
                                    t!("settings.admission_hint").to_string(),
                                    cx,
                                ))
                                .child({
                                    let view = cx.entity();
                                    segmented(
                                        "admission",
                                        &[
                                            t!("settings.admission.ask").to_string(),
                                            t!("settings.admission.trusted").to_string(),
                                            t!("settings.admission.deny").to_string(),
                                        ],
                                        match settings.admission {
                                            AdmissionMode::AlwaysAsk => 0,
                                            AdmissionMode::TrustedAuto => 1,
                                            AdmissionMode::DenyAll => 2,
                                        },
                                        Rc::new(move |i, _, _, app| {
                                            view.update(app, |this, cx| {
                                                let mode = match i {
                                                    1 => AdmissionMode::TrustedAuto,
                                                    2 => AdmissionMode::DenyAll,
                                                    _ => AdmissionMode::AlwaysAsk,
                                                };
                                                if this.persist_settings(|s| s.admission = mode) {
                                                    this.set_status(
                                                        t!("status.admission_mode_updated")
                                                            .to_string(),
                                                        StatusTone::Ok,
                                                    );
                                                }
                                                cx.notify();
                                            });
                                        }),
                                        cx,
                                    )
                                })
                                .child(
                                    div()
                                        .p_3()
                                        .rounded(px(16.))
                                        .bg(colors.accent.opacity(0.08))
                                        .text_size(px(12.))
                                        .text_color(colors.muted_foreground)
                                        .child(
                                            t!(match settings.admission {
                                                AdmissionMode::AlwaysAsk =>
                                                    "settings.admission.ask_hint",
                                                AdmissionMode::TrustedAuto =>
                                                    "settings.admission.trusted_hint",
                                                AdmissionMode::DenyAll =>
                                                    "settings.admission.deny_hint",
                                            })
                                            .to_string(),
                                        ),
                                ),
                        ),
                    );
            }
            3 => {
                content = content
                    .child(header(
                        "wifi",
                        "settings.sharing",
                        "settings.sharing_description",
                    ))
                    .child(
                        form_group(cx)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_4()
                                    .p_5()
                                    .child(
                                        setting_label(
                                            t!("settings.vnc_enabled").to_string(),
                                            format!(
                                                "Apple Remote Desktop · VNC · TCP {}",
                                                settings.vnc_port
                                            ),
                                            cx,
                                        )
                                        .flex_1(),
                                    )
                                    .child(
                                        Switch::new("vnc-enabled")
                                            .checked(settings.vnc_enabled)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                if this.persist_settings(|s| {
                                                    s.vnc_enabled = !s.vnc_enabled
                                                }) {
                                                    this.set_status(
                                                        t!("status.vnc_updated").to_string(),
                                                        StatusTone::Ok,
                                                    );
                                                }
                                                cx.notify();
                                            })),
                                    ),
                            )
                            .when(settings.vnc_enabled, |el| {
                                el.child(Divider::horizontal()).child(
                                    row()
                                        .child(setting_label(
                                            t!("settings.vnc_password").to_string(),
                                            t!("settings.vnc_hint").to_string(),
                                            cx,
                                        ))
                                        .child(form_input(&self.vnc_password_input).mask_toggle())
                                        .child(
                                            div().flex().justify_end().child(
                                                Button::new("save-vnc")
                                                    .label(t!("action.save").to_string())
                                                    .primary()
                                                    .h(px(36.))
                                                    .disabled(
                                                        self.vnc_password_input
                                                            .read(cx)
                                                            .value()
                                                            .as_str()
                                                            == settings.vnc_password,
                                                    )
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.save_vnc(cx)
                                                    })),
                                            ),
                                        ),
                                )
                            }),
                    )
                    .child(
                        div()
                            .p_4()
                            .rounded(px(12.))
                            .bg(colors.accent.opacity(0.08))
                            .text_size(px(12.))
                            .text_color(colors.muted_foreground)
                            .child(t!("settings.sharing_scope_hint").to_string()),
                    );
            }
            _ => {
                content = content
                    .child(header(
                        "info",
                        "settings.system",
                        "settings.system_description",
                    ))
                    .child(
                        form_group(cx).child(
                            row()
                                .child(setting_label(
                                    t!("settings.data_dir").to_string(),
                                    t!("settings.data_hint").to_string(),
                                    cx,
                                ))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .w_full()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .px_3()
                                                .py_2()
                                                .rounded(px(8.))
                                                .bg(colors.background)
                                                .text_size(px(11.))
                                                .font_family(cx.theme().mono_font_family.clone())
                                                .text_color(colors.muted_foreground)
                                                .truncate()
                                                .child(
                                                    self.engine.data_dir().display().to_string(),
                                                ),
                                        )
                                        .child(
                                            Button::new("open-data-dir")
                                                .icon(icon_16("folder"))
                                                .label(t!("action.open").to_string())
                                                .outline()
                                                .h(px(36.))
                                                .flex_shrink_0()
                                                .on_click(cx.listener(|this, _, _, _| {
                                                    let _ = std::process::Command::new("open")
                                                        .arg(this.engine.data_dir())
                                                        .spawn();
                                                })),
                                        ),
                                ),
                        ),
                    )
                    .child(
                        form_group(cx).child(
                            row()
                                .child(setting_label(
                                    t!("settings.logs").to_string(),
                                    t!("settings.logs_hint").to_string(),
                                    cx,
                                ))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .w_full()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .px_3()
                                                .py_2()
                                                .rounded(px(8.))
                                                .bg(colors.background)
                                                .text_size(px(11.))
                                                .font_family(cx.theme().mono_font_family.clone())
                                                .text_color(colors.muted_foreground)
                                                .truncate()
                                                .child(self.engine.data_dir().join("logs").display().to_string()),
                                        )
                                        .child(
                                            Button::new("open-logs-dir")
                                                .icon(icon_16("folder"))
                                                .label(t!("settings.open_logs").to_string())
                                                .outline()
                                                .h(px(36.))
                                                .flex_shrink_0()
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    let dir = this.engine.data_dir().join("logs");
                                                    let result = std::fs::create_dir_all(&dir).and_then(|()| {
                                                        std::process::Command::new("open").arg(&dir).spawn()
                                                    });
                                                    if let Err(error) = result {
                                                        tracing::warn!(%error, "could not open logs folder");
                                                        this.set_status(
                                                            t!("settings.open_logs_failed", err = error.to_string()),
                                                            StatusTone::Err,
                                                        );
                                                        cx.notify();
                                                    }
                                                })),
                                        ),
                                ),
                        ),
                    )
                    .child(self.render_update_section(cx));
            }
        }
        div()
            .id("settings")
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_6()
                    .w_full()
                    .max_w(px(760.))
                    .mx_auto()
                    .p_6()
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(24.))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child(t!("settings.title").to_string()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(colors.muted_foreground)
                                            .child(t!("settings.subtitle").to_string()),
                                    ),
                            )
                            .child(
                                Button::new("close-settings")
                                    .icon(icon_16("x"))
                                    .outline()
                                    .small()
                                    .tooltip(t!("action.back").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.settings_open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(navigation)
                    .flex_shrink_0()
                    .pb_4(),
            )
            .child(
                div()
                    .id(("settings-content", self.settings_section))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(760.))
                            .mx_auto()
                            .px_6()
                            .pb_6()
                            .pt_2()
                            .child(content),
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
                .child(
                    Spinner::new()
                        .icon(icon_16("loader-circle"))
                        .color(colors.muted_foreground),
                )
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
            .child(group_title(t!("update.section").to_string(), cx))
            .child(
                form_group(cx)
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
                                    .small()
                                    .checked(auto_check)
                                    .on_click(cx.listener(|this, _checked, _w, cx| {
                                        let on = !this.engine.settings().update_check_enabled;
                                        this.persist_settings(|s| s.update_check_enabled = on);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(state_block),
            )
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let tone = match self.status_tone {
            StatusTone::Err => colors.danger,
            StatusTone::Warn => colors.warning,
            StatusTone::Ok => colors.success,
            StatusTone::Info => colors.muted_foreground,
        };
        div()
            .min_h(px(32.))
            .flex_shrink_0()
            .px_4()
            .py_2()
            .flex()
            .items_center()
            .gap_2()
            .bg(colors.sidebar)
            .border_t_1()
            .border_color(colors.border)
            .child(dot(tone))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(11.))
                    .text_color(tone)
                    .child(self.status.clone()),
            )
    }

    fn render_pin_dialog(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let dialog = self.pin_dialog.as_ref()?;
        let colors = cx.theme().colors;
        let card = div()
            .w(px(360.))
            .p_6()
            .rounded(px(16.))
            .bg(colors.popover)
            .border_1()
            .border_color(colors.border)
            .shadow(theme::popup_shadow(cx.theme().is_dark()))
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
                            .text_size(px(32.))
                            .font_family(cx.theme().mono_font_family.clone())
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(colors.foreground)
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
                .child(form_input(&self.pin_input))
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
                                .disabled({
                                    let pin = self.pin_input.read(cx).value();
                                    let pin = pin.trim();
                                    pin.len() != 6 || !pin.bytes().all(|b| b.is_ascii_digit())
                                })
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
            .rounded(px(16.))
            .bg(colors.popover)
            .border_1()
            .border_color(colors.border)
            .shadow(theme::popup_shadow(cx.theme().is_dark()))
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
                    .child(device_glyph(32., &colors))
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
                            .outline()
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
            |this, delta| this.mt(px(-8.) + delta * px(8.)),
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
        let connection_visible = self.pin_dialog.is_none() && self.admission.is_none();
        if let Some(dialog) = &self.connection_dialog {
            dialog.update(cx, |form, cx| {
                form.set_obscured(!connection_visible, &self.focus, window, cx)
            });
        }
        let colors = cx.theme().colors;
        let detail = if self.settings_open {
            self.render_settings(cx).into_any_element()
        } else {
            match self.selected.clone() {
                Some(Selection::Device(fp)) => match self.devices.get(&fp).cloned() {
                    Some(row) => self.render_device_detail(&fp, &row, cx).into_any_element(),
                    None => self.render_empty_detail(cx).into_any_element(),
                },
                Some(Selection::Saved(id)) => {
                    match self.saved.iter().find(|s| s.id == id).cloned() {
                        Some(entry) => self.render_saved_detail(&entry, cx).into_any_element(),
                        None => self.render_empty_detail(cx).into_any_element(),
                    }
                }
                None => self.render_empty_detail(cx).into_any_element(),
            }
        };

        div()
            .key_context("Home")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &HomeSettings, window, cx| {
                if this.pin_dialog.is_none()
                    && this.admission.is_none()
                    && this.connection_dialog.is_none()
                {
                    this.settings_open = !this.settings_open;
                    window.focus(&this.focus);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &HomeConnect, window, cx| {
                if this.pin_dialog.is_none()
                    && this.admission.is_none()
                    && this.connecting.is_none()
                {
                    this.open_connection_dialog(None, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &HomeSearch, window, cx| {
                if this.pin_dialog.is_none()
                    && this.admission.is_none()
                    && this.connection_dialog.is_none()
                {
                    this.search_input
                        .update(cx, |input, cx| input.focus(window, cx));
                    cx.notify();
                }
            }))
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
                    window.focus(&this.focus);
                    cx.notify();
                } else if let Some(dialog) = &this.connection_dialog {
                    dialog.update(cx, |form, cx| form.back(window, cx));
                } else if this.settings_open {
                    this.settings_open = false;
                    window.focus(&this.focus);
                    cx.notify();
                }
            }))
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().transparent)
            .text_color(colors.foreground)
            .child(self.render_title_bar(cx))
            .when(
                self.connecting.is_some() && self.connection_dialog.is_none(),
                |el| {
                    el.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_5()
                            .py_3()
                            .flex_shrink_0()
                            .bg(colors.accent.opacity(0.08))
                            .border_b_1()
                            .border_color(colors.border)
                            .child(Spinner::new().icon(icon_16("loader-circle")).small())
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(div().text_size(px(13.)).child(
                                        super::connection::stage_label(self.connection_stage),
                                    ))
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(colors.muted_foreground)
                                            .truncate()
                                            .child(self.connecting.clone().unwrap_or_default()),
                                    ),
                            )
                            .child(
                                Button::new("cancel-pending-session")
                                    .outline()
                                    .label(t!("connection.cancel_attempt").to_string())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.cancel_connection(cx)),
                                    ),
                            ),
                    )
                },
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.render_sidebar(window, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .bg(colors.background)
                            .overflow_hidden()
                            .child(detail),
                    ),
            )
            .child(self.render_status_bar(cx))
            .children(
                self.connection_dialog
                    .as_ref()
                    .filter(|_| connection_visible)
                    .map(|dialog| {
                        modal_overlay(
                            "connection-overlay",
                            self.dialog_seq,
                            div()
                                .w(px(480.))
                                .max_w(window.viewport_size().width - px(48.))
                                .rounded(px(20.))
                                .bg(colors.popover)
                                .border_1()
                                .border_color(colors.border)
                                .shadow(theme::popup_shadow(cx.theme().is_dark()))
                                .child(dialog.clone()),
                            cx,
                        )
                    }),
            )
            .children(self.render_pin_dialog(cx))
            .children(self.render_admission_dialog(cx))
    }
}

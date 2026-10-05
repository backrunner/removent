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
use removent_core::{AdmissionMode, Language, Theme as ThemePref, UpdateChannel};
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

// Content aligns with the search field; row highlights extend into the gutter.
// Row padding accounts for the 1px focus/selection border.
const SIDEBAR_INSET: f32 = 16.;
const SIDEBAR_ROW_INSET: f32 = 8.;

#[derive(Clone)]
struct DeviceRow {
    name: String,
    addr: SocketAddr,
    protocol: ConnectionProtocol,
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
    Trust {
        destination: String,
        relay: bool,
        tx: oneshot::Sender<bool>,
    },
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
    save_warning: Option<String>,
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
    auth_mode: removent_core::AuthenticationMode,
    auth_password_input: Entity<InputState>,
    search_input: Entity<InputState>,
    connection_dialog: Option<Entity<ConnectionDialog>>,
    connection_subscription: Option<gpui::Subscription>,
    connection_save_task: Option<gpui::Task<()>>,
    settings_open: bool,
    settings_section: usize,
    device_name_input: Entity<InputState>,
    vnc_password_input: Entity<InputState>,
    /// Auto-update state machine snapshot (engine → UiEvent::UpdateStatus).
    update_status: UpdateStatus,
    cloud_sync_status: serde_json::Value,
    my_fp_short: String,
    focus: FocusHandle,
    /// Liveness flag for the ui-event-bridge thread: cleared on drop so the thread can
    /// exit even when the engine (the channel sender) outlives this window.
    bridge_alive: Arc<AtomicBool>,
}

/// Grouped PIN display: 6 digits → "123 456".
fn group_pin(pin: &str) -> String {
    if pin.len() == 12 && pin.is_ascii() {
        format!("{} {}", &pin[..6], &pin[6..])
    } else if pin.len() == 6 && pin.is_ascii() {
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
        .px(px(SIDEBAR_ROW_INSET))
        .pt_3()
        .pb_1()
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

/// A single line that remeasures when its flex allocation changes. GPUI's
/// nowrap text can retain its intrinsic measurement and get clipped instead of
/// ellipsized; a one-line clamp keeps the available width in the layout key.
fn sidebar_line() -> Div {
    div()
        .w_full()
        .min_w_0()
        .whitespace_normal()
        .text_ellipsis()
        .line_clamp(1)
}

/// Shared compact typography for nearby, saved and local device rows.
fn sidebar_labels(width: f32, title: String, subtitle: String, cx: &App) -> Div {
    div()
        .w(px(width))
        .flex_shrink_0()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            sidebar_line()
                .w(px(width))
                .text_size(px(13.))
                .line_height(px(16.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(title),
        )
        .child(
            sidebar_line()
                .w(px(width))
                .text_size(px(11.))
                .line_height(px(14.))
                .text_color(cx.theme().muted_foreground)
                .child(subtitle),
        )
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

impl Drop for HomeView {
    fn drop(&mut self) {
        self.bridge_alive.store(false, Ordering::Relaxed);
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

mod actions;
mod admission;
mod cloud_sync;
mod connections;
mod details;
mod dialogs;
mod events;
mod lifecycle;
mod preferences;
mod render;
mod settings;
mod sidebar;
#[cfg(test)]
mod tests;
mod updates;

//! Viewer session window: immersive picture, floating controls, optional diagnostics.

use crate::engine::{ClientDiagnostics, Engine, VideoFrame};
use crate::ui::motion;
use crate::ui::widgets::*;
use gpui::{
    AnimationExt, App, Bounds, Context, ElementId, FocusHandle, Keystroke, ModifiersChangedEvent,
    MouseButton, ObjectFit, Point, Render, RenderImage, Subscription, Task, TouchPhase, Window,
    WindowOptions, actions, div, img, prelude::*, px, size,
};
use gpui_component::{ActiveTheme, TITLE_BAR_HEIGHT, TitleBar};
use removent_core::latest::Receiver;
use removent_proto::{KeyKind, KeyModifiers, MouseKind, ScrollPhase};
use rust_i18n::t;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

actions!(
    viewer,
    [ViewerEscape, ViewerToggleFullscreen, ViewerToggleInfo]
);

const TOOLBAR_HIDE_AFTER: Duration = Duration::from_millis(800);
/// Moving the mouse to the top window edge (y < 8px, window coordinates) reveals the toolbar.
const TOOLBAR_SHOW_EDGE: f32 = 8.;
/// After revealing, the hide timer keeps refreshing while the mouse stays in the top area
/// (including the toolbar itself). Window coordinate semantics: TitleBar 34px + toolbar top
/// 42px + ~28px height + 12px slack; the threshold must cover the entire toolbar button area,
/// otherwise hovering a button would hide the toolbar and mis-intercept the click.
const TOOLBAR_HOVER_AREA: f32 = 116.;

/// (modifier bit, left-variant macOS virtual key code); shared by
/// forward_modifiers (state diffing) and release_held_inputs (focus loss).
const MOD_KEYS: [(KeyModifiers, u16); 6] = [
    (KeyModifiers::SHIFT, 0x38),
    (KeyModifiers::CONTROL, 0x3B),
    (KeyModifiers::OPTION, 0x3A),
    (KeyModifiers::COMMAND, 0x37),
    (KeyModifiers::FUNCTION, 0x3F),
    (KeyModifiers::CAPS_LOCK, 0x39),
];

pub struct ViewerView {
    engine: Engine,
    session_generation: usize,
    current: Option<Arc<RenderImage>>,
    width: u32,
    height: u32,
    fps_counter: u32,
    fps_shown: f64,
    last_fps_tick: Instant,
    info_visible: bool,
    info_scroll: gpui::ScrollHandle,
    started: Instant,
    last_frame: Option<Instant>,
    diagnostics: ClientDiagnostics,
    received_bytes: u64,
    received_frames: u64,
    receive_rate: f64,
    receive_fps: f64,
    peer_name: String,
    toolbar_until: Option<Instant>,
    /// Bumped each time the toolbar transitions hidden → visible: folded into the animation
    /// element id so the entry transition replays on every appearance.
    toolbar_seq: u64,
    ended: bool,
    focus: FocusHandle,
    /// Currently held remote mouse buttons (bit0 left, bit1 right, bit2 middle),
    /// mirrored into every outbound mouse event.
    buttons: u8,
    /// Modifier state last forwarded to the host; ModifiersChanged events are diffed
    /// against it to derive per-modifier FlagsChanged key events.
    sent_modifiers: KeyModifiers,
    /// Virtual key codes forwarded as Down but not yet released as Up. macOS
    /// delivers key-up only to the key window, so these must be released
    /// explicitly when the window loses focus.
    pressed_keys: HashSet<u16>,
    /// Last forwarded remote cursor position (frame pixels); reused as the
    /// position for focus-loss button releases.
    last_mouse: Option<(f32, f32)>,
    /// Bumped each time the toolbar hide timer is (re-)armed; stale timers
    /// compare against it and no-op.
    toolbar_hide_seq: u64,
    /// Pending local-clipboard clear (drop = cancel); armed on window focus loss.
    clip_clear_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Drop for ViewerView {
    fn drop(&mut self) {
        // Ensure the session task is terminated when the window is closed
        // (traffic lights / Esc / button).
        self.engine.disconnect_client_if(self.session_generation);
    }
}

/// Open the Viewer window and take over the frame channel.
pub fn open_viewer_window(
    engine: Engine,
    frames_rx: Receiver<VideoFrame>,
    peer_name: String,
    cx: &mut App,
) -> Result<(), String> {
    let bounds = Bounds::centered(None, size(px(1280.), px(720.)), cx);
    let handle = cx.open_window(
        WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
            titlebar: Some(TitleBar::title_bar_options()),
            window_min_size: Some(size(px(480.), px(320.))),
            ..Default::default()
        },
        move |window, cx| {
            let view = cx.new(|cx| ViewerView::new(engine, frames_rx, peer_name, window, cx));
            cx.new(|cx| gpui_component::Root::new(gpui::AnyView::from(view), window, cx))
        },
    );
    handle.map(|_| ()).map_err(|e| e.to_string())
}

mod input;
mod keys;
mod lifecycle;
mod render;
#[cfg(test)]
mod tests;
mod toolbar;

use keys::{
    format_duration, format_ms, gpui_modifiers, is_viewer_shortcut, key_modifiers, mac_vk_of_key,
};

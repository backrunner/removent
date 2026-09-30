//! Input injection abstraction: real CGEventPost and a test recorder.

use removent_proto::{KeyKind, KeyModifiers, MouseKind, ScrollPhase};

/// Mouse button bits carried in `MouseEvent.buttons` (client convention:
/// bit0=left, bit1=right, bit2=middle).
pub const BUTTON_LEFT: u8 = 1 << 0;
pub const BUTTON_RIGHT: u8 = 1 << 1;
pub const BUTTON_MIDDLE: u8 = 1 << 2;

/// Host-side input injection surface.
pub trait InputSink: Send + Sync {
    fn mouse(
        &self,
        display_id: u64,
        x_px: f32,
        y_px: f32,
        buttons: u8,
        kind: MouseKind,
    ) -> Result<(), String>;
    fn key(
        &self,
        vk_code: u16,
        modifiers: KeyModifiers,
        kind: KeyKind,
        unicode: Option<char>,
    ) -> Result<(), String>;
    fn scroll(
        &self,
        display_id: u64,
        dx_mm: f32,
        dy_mm: f32,
        phase: ScrollPhase,
    ) -> Result<(), String>;
    /// Tells the sink the capture-frame pixel dimensions the wire coordinates
    /// refer to (called when capture starts or restarts with new dimensions).
    /// Default no-op (test recorders don't need it).
    fn set_capture_dims(&self, _width_px: u32, _height_px: u32) {}
}

/// Maps a Moved event to the matching drag kind when a button is held; all
/// other kinds pass through unchanged.
pub fn drag_kind_for(buttons: u8, kind: MouseKind) -> MouseKind {
    if kind != MouseKind::Moved {
        return kind;
    }
    if buttons & BUTTON_LEFT != 0 {
        MouseKind::LeftDragged
    } else if buttons & BUTTON_RIGHT != 0 {
        MouseKind::RightDragged
    } else if buttons & BUTTON_MIDDLE != 0 {
        MouseKind::MiddleDragged
    } else {
        MouseKind::Moved
    }
}

/// Resolves the target display's geometry, falling back to the main display
/// when the id is unknown (e.g. the display was just unplugged).
fn display_geometry(display_id: u64) -> Option<removent_input::DisplayGeometry> {
    removent_input::DisplayGeometry::by_id(display_id)
        .or_else(removent_input::DisplayGeometry::main)
}

/// Real implementation: CGEvent injection (requires Accessibility permission).
pub struct RealInputSink {
    /// Capture-frame pixel dims the peer's wire coordinates refer to. The
    /// capture is capped at 1920×1080, so on Retina displays capture pixels
    /// are smaller than physical pixels and must be rescaled before the
    /// scale/origin conversion. None = wire coords are physical pixels.
    capture_dims: std::sync::Mutex<Option<(u32, u32)>>,
}

impl RealInputSink {
    pub fn new() -> Self {
        Self {
            capture_dims: std::sync::Mutex::new(None),
        }
    }
}

impl Default for RealInputSink {
    fn default() -> Self {
        Self::new()
    }
}

impl InputSink for RealInputSink {
    fn mouse(
        &self,
        display_id: u64,
        x_px: f32,
        y_px: f32,
        buttons: u8,
        kind: MouseKind,
    ) -> Result<(), String> {
        let kind = drag_kind_for(buttons, kind);
        // Wire coords are capture-frame pixels of the target display; CGEvent
        // expects global display points. Rescale capture px → display physical
        // px (the capture may be downscaled), then divide by the display scale
        // and translate by the display's global origin.
        let (x_pt, y_pt) = match display_geometry(display_id) {
            Some(g) => match *self.capture_dims.lock().unwrap() {
                Some((cw, ch)) => {
                    g.capture_px_to_global_pt(x_px as f64, y_px as f64, cw as f64, ch as f64)
                }
                None => g.px_to_global_pt(x_px as f64, y_px as f64),
            },
            None => (x_px as f64, y_px as f64),
        };
        removent_input::inject_mouse(kind, x_pt, y_pt)
    }
    fn key(
        &self,
        vk_code: u16,
        modifiers: KeyModifiers,
        kind: KeyKind,
        unicode: Option<char>,
    ) -> Result<(), String> {
        removent_input::inject_key(vk_code, modifiers, kind, unicode)
    }
    fn scroll(
        &self,
        display_id: u64,
        dx_mm: f32,
        dy_mm: f32,
        phase: ScrollPhase,
    ) -> Result<(), String> {
        // mm → px using the target display's real DPI (96 × scale), falling
        // back to 96dpi when the display cannot be resolved.
        let dpi = display_geometry(display_id)
            .map(|g| g.dpi())
            .unwrap_or(96.0);
        let px_per_mm = dpi / 25.4;
        removent_input::inject_scroll_pixels(
            dx_mm as f64 * px_per_mm,
            dy_mm as f64 * px_per_mm,
            phase,
        )
    }

    fn set_capture_dims(&self, width_px: u32, height_px: u32) {
        *self.capture_dims.lock().unwrap() = Some((width_px, height_px));
    }
}

/// Tracks pressed keys and held mouse buttons so the host can inject the
/// matching releases when a session tears down — including abrupt network
/// loss, where the client never gets to send the release events.
#[derive(Default)]
pub struct InputReleaseTracker {
    keys: std::collections::HashSet<u16>,
    /// Modifier keys currently held (tracked via FlagsChanged direction
    /// inference, since macOS FlagsChanged has no explicit down/up).
    mod_keys: std::collections::HashSet<u16>,
    buttons: u8,
    last_pos: Option<(u64, f32, f32)>,
}

impl InputReleaseTracker {
    pub fn note_key(&mut self, vk_code: u16, modifiers: KeyModifiers, kind: KeyKind) {
        match kind {
            KeyKind::Down => {
                self.keys.insert(vk_code);
            }
            KeyKind::Up => {
                self.keys.remove(&vk_code);
            }
            // FlagsChanged carries no explicit direction: infer it from whether
            // the event's modifiers include this key's flag (same mapping as
            // inject_key uses), so release_all can unstick held modifiers.
            KeyKind::FlagsChanged => {
                if let Some(bit) = removent_input::modifier_of_vk(vk_code) {
                    if modifiers.contains(bit) {
                        self.mod_keys.insert(vk_code);
                    } else {
                        self.mod_keys.remove(&vk_code);
                    }
                }
            }
        }
    }

    pub fn note_mouse(&mut self, display_id: u64, x_px: f32, y_px: f32, kind: MouseKind) {
        self.last_pos = Some((display_id, x_px, y_px));
        match kind {
            MouseKind::LeftDown => self.buttons |= BUTTON_LEFT,
            MouseKind::LeftUp => self.buttons &= !BUTTON_LEFT,
            MouseKind::RightDown => self.buttons |= BUTTON_RIGHT,
            MouseKind::RightUp => self.buttons &= !BUTTON_RIGHT,
            MouseKind::MiddleDown => self.buttons |= BUTTON_MIDDLE,
            MouseKind::MiddleUp => self.buttons &= !BUTTON_MIDDLE,
            _ => {}
        }
    }

    /// Keep teardown's button-up at the same physical position when the viewer
    /// acknowledges a resize without sending another mouse event.
    pub fn rescale_position(&mut self, old: (u32, u32), new: (u32, u32)) {
        if old.0 > 0
            && old.1 > 0
            && let Some((_, x, y)) = &mut self.last_pos
        {
            *x *= new.0 as f32 / old.0 as f32;
            *y *= new.1 as f32 / old.1 as f32;
        }
    }

    /// Injects key-up / button-up for everything currently held, then clears
    /// the tracked state. Injection errors are ignored (teardown path).
    pub fn release_all(&mut self, sink: &dyn InputSink) {
        let keys: Vec<u16> = self.keys.drain().collect();
        for vk in keys {
            let _ = sink.key(vk, KeyModifiers::empty(), KeyKind::Up, None);
        }
        // Held modifiers: a FlagsChanged with the key's flag cleared reads as a
        // release on the receiving end (inject_key infers direction from flags).
        let mod_keys: Vec<u16> = self.mod_keys.drain().collect();
        for vk in mod_keys {
            let _ = sink.key(vk, KeyModifiers::empty(), KeyKind::FlagsChanged, None);
        }
        let held = std::mem::take(&mut self.buttons);
        if held != 0
            && let Some((display_id, x, y)) = self.last_pos
        {
            for (bit, up) in [
                (BUTTON_LEFT, MouseKind::LeftUp),
                (BUTTON_RIGHT, MouseKind::RightUp),
                (BUTTON_MIDDLE, MouseKind::MiddleUp),
            ] {
                if held & bit != 0 {
                    let _ = sink.mouse(display_id, x, y, 0, up);
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecordedInput {
    Mouse {
        display_id: u64,
        x: f32,
        y: f32,
        buttons: u8,
        kind: MouseKind,
    },
    Key {
        vk: u16,
        mods: KeyModifiers,
        kind: KeyKind,
        unicode: Option<char>,
    },
    Scroll {
        display_id: u64,
        dx: f32,
        dy: f32,
        phase: ScrollPhase,
    },
}

/// Test recorder: records all injection requests.
#[derive(Default)]
pub struct RecorderInputSink {
    pub events: std::sync::Mutex<Vec<RecordedInput>>,
}

impl InputSink for RecorderInputSink {
    fn mouse(
        &self,
        display_id: u64,
        x_px: f32,
        y_px: f32,
        buttons: u8,
        kind: MouseKind,
    ) -> Result<(), String> {
        self.events.lock().unwrap().push(RecordedInput::Mouse {
            display_id,
            x: x_px,
            y: y_px,
            buttons,
            kind,
        });
        Ok(())
    }
    fn key(
        &self,
        vk_code: u16,
        modifiers: KeyModifiers,
        kind: KeyKind,
        unicode: Option<char>,
    ) -> Result<(), String> {
        self.events.lock().unwrap().push(RecordedInput::Key {
            vk: vk_code,
            mods: modifiers,
            kind,
            unicode,
        });
        Ok(())
    }
    fn scroll(
        &self,
        display_id: u64,
        dx_mm: f32,
        dy_mm: f32,
        phase: ScrollPhase,
    ) -> Result<(), String> {
        self.events.lock().unwrap().push(RecordedInput::Scroll {
            display_id,
            dx: dx_mm,
            dy: dy_mm,
            phase,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_after_resize_keeps_the_same_relative_position() {
        let sink = RecorderInputSink::default();
        let mut tracker = InputReleaseTracker::default();
        tracker.note_mouse(1, 160., 120., MouseKind::LeftDown);
        tracker.rescale_position((320, 240), (160, 120));
        tracker.release_all(&sink);
        assert!(matches!(
            sink.events.lock().unwrap()[0],
            RecordedInput::Mouse {
                x: 80.,
                y: 60.,
                kind: MouseKind::LeftUp,
                ..
            }
        ));
    }

    #[test]
    fn moved_with_left_button_becomes_left_drag() {
        assert_eq!(
            drag_kind_for(BUTTON_LEFT, MouseKind::Moved),
            MouseKind::LeftDragged
        );
        assert_eq!(
            drag_kind_for(BUTTON_RIGHT, MouseKind::Moved),
            MouseKind::RightDragged
        );
        assert_eq!(
            drag_kind_for(BUTTON_MIDDLE, MouseKind::Moved),
            MouseKind::MiddleDragged
        );
        assert_eq!(drag_kind_for(0, MouseKind::Moved), MouseKind::Moved);
    }

    #[test]
    fn non_moved_kinds_pass_through() {
        assert_eq!(
            drag_kind_for(BUTTON_LEFT, MouseKind::LeftDown),
            MouseKind::LeftDown
        );
        assert_eq!(drag_kind_for(0, MouseKind::RightUp), MouseKind::RightUp);
    }

    #[test]
    fn release_tracker_replays_held_keys_and_buttons() {
        let sink = RecorderInputSink::default();
        let mut t = InputReleaseTracker::default();
        t.note_key(0x00, KeyModifiers::empty(), KeyKind::Down);
        t.note_key(0x01, KeyModifiers::empty(), KeyKind::Down);
        t.note_key(0x01, KeyModifiers::empty(), KeyKind::Up); // released before teardown: not replayed
        t.note_mouse(7, 10.0, 20.0, MouseKind::LeftDown);
        t.note_mouse(7, 12.0, 24.0, MouseKind::Moved);
        t.release_all(&sink);

        let events = sink.events.lock().unwrap();
        assert!(events.iter().any(|e| matches!(
            e,
            RecordedInput::Key {
                vk: 0x00,
                kind: KeyKind::Up,
                ..
            }
        )));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, RecordedInput::Key { vk: 0x01, .. }))
        );
        assert!(events.iter().any(|e| matches!(
            e,
            RecordedInput::Mouse {
                display_id: 7,
                kind: MouseKind::LeftUp,
                x,
                y,
                ..
            } if *x == 12.0 && *y == 24.0
        )));
        drop(events);

        // State is cleared: a second release_all injects nothing.
        t.release_all(&sink);
        assert_eq!(sink.events.lock().unwrap().len(), 2);
    }

    #[test]
    fn release_tracker_replays_held_modifiers() {
        let sink = RecorderInputSink::default();
        let mut t = InputReleaseTracker::default();
        // Shift down (0x38 with SHIFT set), Command down (0x37), Shift up again.
        t.note_key(0x38, KeyModifiers::SHIFT, KeyKind::FlagsChanged);
        t.note_key(0x37, KeyModifiers::COMMAND, KeyKind::FlagsChanged);
        t.note_key(0x38, KeyModifiers::empty(), KeyKind::FlagsChanged);
        t.release_all(&sink);

        let events = sink.events.lock().unwrap();
        // Only Command is still held: exactly one release, as a FlagsChanged
        // with the flag cleared (direction inferred as up by inject_key).
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            RecordedInput::Key {
                vk: 0x37,
                mods,
                kind: KeyKind::FlagsChanged,
                ..
            } if mods == KeyModifiers::empty()
        ));
    }
}

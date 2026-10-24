//! Keyboard and scroll wheel injection.

use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::CGEventSourceStateID;
use removent_proto::{KeyKind, KeyModifiers, ScrollPhase};

/// Sentinel vk for unicode keys with no physical key on the client layout
/// (é/ü and friends): the viewer sends it instead of vk 0 so it never
/// collides with 'a' (real mac vk 0x00) in the viewer's/host's pressed-key
/// tracking sets. Injection drops the keycode and posts the unicode payload
/// only.
pub const UNICODE_ONLY_VK: u16 = 0xFFFF;

/// Text must not override physical control/navigation keys. GPUI supplies LF
/// for Return, but loginwindow expects the native Return event (CR).
pub fn key_text(vk: u16, mods: KeyModifiers, unicode: Option<char>) -> Option<char> {
    if mods.intersects(KeyModifiers::COMMAND | KeyModifiers::CONTROL)
        || matches!(vk, 0x24 | 0x30 | 0x33 | 0x35 | 0x4C | 0x72..=0x7E)
    {
        return None;
    }
    unicode.filter(|c| !c.is_control() && !('\u{f700}'..='\u{f8ff}').contains(c))
}

fn source() -> Result<core_graphics::event_source::CGEventSource, String> {
    core_graphics::event_source::CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|_| "event source create failed".to_string())
}

pub(crate) fn flags_of(mods: KeyModifiers) -> CGEventFlags {
    let mut f = CGEventFlags::CGEventFlagNull;
    if mods.contains(KeyModifiers::SHIFT) {
        f |= CGEventFlags::CGEventFlagShift;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        f |= CGEventFlags::CGEventFlagControl;
    }
    if mods.contains(KeyModifiers::OPTION) {
        f |= CGEventFlags::CGEventFlagAlternate;
    }
    if mods.contains(KeyModifiers::COMMAND) {
        f |= CGEventFlags::CGEventFlagCommand;
    }
    // CAPS_LOCK (AlphaShift) is a lock state, not a modifier press, and is not
    // attached to ordinary key events; NUMPAD is only a key-source marker, not
    // an Fn modifier, so it does not map to SecondaryFn.
    if mods.contains(KeyModifiers::FUNCTION) {
        f |= CGEventFlags::CGEventFlagSecondaryFn;
    }
    f
}

/// FlagsChanged only: CapsLock/Fn's own state changes need to be mapped into flags.
fn flags_of_flags_changed(mods: KeyModifiers) -> CGEventFlags {
    let mut f = flags_of(mods);
    if mods.contains(KeyModifiers::CAPS_LOCK) {
        f |= CGEventFlags::CGEventFlagAlphaShift;
    }
    if mods.contains(KeyModifiers::NUMPAD) {
        f |= CGEventFlags::CGEventFlagSecondaryFn;
    }
    f
}

/// FlagsChanged vk_code → corresponding `KeyModifiers` bit (the public,
/// CG-free counterpart of [`modifier_flag_of_vk`], used by the host's release
/// tracker to infer press/release direction).
pub fn modifier_of_vk(vk_code: u16) -> Option<KeyModifiers> {
    // macOS virtual key codes; mirrors modifier_flag_of_vk.
    match vk_code {
        0x37 | 0x36 => Some(KeyModifiers::COMMAND), // L/R Command
        0x38 | 0x3C => Some(KeyModifiers::SHIFT),   // L/R Shift
        0x3A | 0x3D => Some(KeyModifiers::OPTION),  // L/R Option
        0x3B | 0x3E => Some(KeyModifiers::CONTROL), // L/R Control
        0x39 => Some(KeyModifiers::CAPS_LOCK),      // Caps Lock
        0x3F => Some(KeyModifiers::FUNCTION),       // Fn
        _ => None,
    }
}

/// FlagsChanged vk_code → corresponding modifier flag (used to infer press/release direction).
fn modifier_flag_of_vk(vk_code: u16) -> Option<CGEventFlags> {
    // macOS virtual key codes.
    match vk_code {
        0x37 | 0x36 => Some(CGEventFlags::CGEventFlagCommand), // L/R Command
        0x38 | 0x3C => Some(CGEventFlags::CGEventFlagShift),   // L/R Shift
        0x3A | 0x3D => Some(CGEventFlags::CGEventFlagAlternate), // L/R Option
        0x3B | 0x3E => Some(CGEventFlags::CGEventFlagControl), // L/R Control
        0x39 => Some(CGEventFlags::CGEventFlagAlphaShift),     // Caps Lock
        0x3F => Some(CGEventFlags::CGEventFlagSecondaryFn),    // Fn
        _ => None,
    }
}

/// Injects a keyboard event (vk_code is a macOS virtual key code).
///
/// When `unicode` is set on a Down/Up event without Command/Control held, the
/// character is attached via CGEventKeyboardSetUnicodeString so text follows
/// the *client's* layout regardless of the host's keyboard layout. Control and
/// navigation keys (Command/Control held, or no unicode payload) keep plain
/// vk_code semantics.
pub fn inject_key(
    vk_code: u16,
    modifiers: KeyModifiers,
    kind: KeyKind,
    unicode: Option<char>,
) -> Result<(), String> {
    if let Some(event) = make_key_event(vk_code, modifiers, kind, unicode, false)? {
        event.post(CGEventTapLocation::HID);
    }
    Ok(())
}

/// Per-session repeat and modifier state, also used for mouse/scroll flags.
#[derive(Default)]
pub struct KeyboardInjector {
    pressed: std::collections::HashSet<u16>,
    pub modifiers: KeyModifiers,
}

impl KeyboardInjector {
    pub fn inject(
        &mut self,
        vk: u16,
        mods: KeyModifiers,
        kind: KeyKind,
        text: Option<char>,
    ) -> Result<(), String> {
        if let Some(event) = self.build(vk, mods, kind, text)? {
            event.post(CGEventTapLocation::HID);
        }
        Ok(())
    }

    fn build(
        &mut self,
        vk: u16,
        mods: KeyModifiers,
        kind: KeyKind,
        text: Option<char>,
    ) -> Result<Option<CGEvent>, String> {
        let repeat = kind == KeyKind::Down && self.pressed.contains(&vk);
        let event = make_key_event(vk, mods, kind, text, repeat)?;
        if event.is_some() {
            self.modifiers = mods;
            match kind {
                KeyKind::Down => {
                    self.pressed.insert(vk);
                }
                KeyKind::Up => {
                    self.pressed.remove(&vk);
                }
                KeyKind::FlagsChanged => {}
            }
        }
        Ok(event)
    }
}

fn make_key_event(
    vk_code: u16,
    modifiers: KeyModifiers,
    kind: KeyKind,
    unicode: Option<char>,
    repeat: bool,
) -> Result<Option<CGEvent>, String> {
    let unicode = key_text(vk_code, modifiers, unicode);
    if vk_code == UNICODE_ONLY_VK && unicode.is_none() {
        return Ok(None);
    }
    let flags = if kind == KeyKind::FlagsChanged {
        flags_of_flags_changed(modifiers)
    } else {
        flags_of(modifiers)
    };
    let down = if kind == KeyKind::FlagsChanged {
        modifier_flag_of_vk(vk_code).is_some_and(|f| flags.intersects(f))
    } else {
        kind == KeyKind::Down
    };
    let vk = if vk_code == UNICODE_ONLY_VK {
        0
    } else {
        vk_code
    };
    let event = CGEvent::new_keyboard_event(source()?, vk, down)
        .map_err(|_| "keyboard event create failed".to_string())?;
    if kind == KeyKind::FlagsChanged {
        event.set_type(core_graphics::event::CGEventType::FlagsChanged);
    }
    event.set_flags(flags);
    event.set_integer_value_field(
        core_graphics::event::EventField::KEYBOARD_EVENT_AUTOREPEAT,
        i64::from(repeat),
    );
    if let Some(c) = unicode {
        let mut buf = [0u16; 2];
        event.set_string_from_utf16_unchecked(c.encode_utf16(&mut buf));
    }
    Ok(Some(event))
}

/// Construction check (not posted), using the same path as injection.
pub fn build_key(vk_code: u16, modifiers: KeyModifiers, kind: KeyKind) -> Result<(), String> {
    make_key_event(vk_code, modifiers, kind, None, false).map(|_| ())
}

/// `kCGScrollWheelEventScrollPhase` (not exported by core-graphics).
const SCROLL_WHEEL_EVENT_SCROLL_PHASE: core_graphics::event::CGEventField = 99;
const SCROLL_WHEEL_EVENT_FIXED_PT_DELTA_AXIS_1: core_graphics::event::CGEventField = 93;
const SCROLL_WHEEL_EVENT_FIXED_PT_DELTA_AXIS_2: core_graphics::event::CGEventField = 94;

fn cg_scroll_phase(phase: ScrollPhase) -> i64 {
    // CGScrollPhase: None=0, Began=1, Changed=2, Ended=4 (IOHID phase semantics).
    match phase {
        ScrollPhase::Began => 1,
        ScrollPhase::Changed => 2,
        ScrollPhase::Ended => 4,
    }
}

/// Injects a pixel-level scroll wheel event. The protocol-level millimeter
/// amount is converted by the host engine using the target display's DPI before
/// being passed in. PIXEL units and fixed-point deltas retain fractional input;
/// `phase` preserves gesture boundaries for the receiving application.
pub fn inject_scroll_pixels(dx_px: f64, dy_px: f64, phase: ScrollPhase) -> Result<(), String> {
    inject_scroll_with_modifiers(dx_px, dy_px, phase, KeyModifiers::empty())
}

pub fn inject_scroll_with_modifiers(
    dx_px: f64,
    dy_px: f64,
    phase: ScrollPhase,
    modifiers: KeyModifiers,
) -> Result<(), String> {
    let event = make_scroll_event(dx_px, dy_px, phase, modifiers)?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

fn make_scroll_event(
    dx_px: f64,
    dy_px: f64,
    phase: ScrollPhase,
    modifiers: KeyModifiers,
) -> Result<CGEvent, String> {
    let event = CGEvent::new_scroll_event(
        source()?,
        core_graphics::event::ScrollEventUnit::PIXEL,
        2,
        -(dy_px.round() as i32),
        -(dx_px.round() as i32),
        0,
    )
    .map_err(|_| "scroll event create failed".to_string())?;
    use core_graphics::event::EventField;
    event.set_flags(flags_of(modifiers));
    // Point-delta fields are integers; writing doubles silently truncates them.
    // Fractional values belong in the documented 16.16 fixed-point fields.
    event.set_integer_value_field(
        EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_1,
        -(dy_px.round() as i64),
    );
    event.set_integer_value_field(
        EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_2,
        -(dx_px.round() as i64),
    );
    event.set_double_value_field(SCROLL_WHEEL_EVENT_FIXED_PT_DELTA_AXIS_1, -dy_px);
    event.set_double_value_field(SCROLL_WHEEL_EVENT_FIXED_PT_DELTA_AXIS_2, -dx_px);
    event.set_integer_value_field(EventField::SCROLL_WHEEL_EVENT_IS_CONTINUOUS, 1);
    event.set_integer_value_field(SCROLL_WHEEL_EVENT_SCROLL_PHASE, cg_scroll_phase(phase));
    Ok(event)
}

/// Construction check (not posted), using the same path as injection.
pub fn build_scroll(dx_px: f64, dy_px: f64, phase: ScrollPhase) -> Result<(), String> {
    make_scroll_event(dx_px, dy_px, phase, KeyModifiers::empty()).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_are_flags_changed_and_repeat_resets_after_release() {
        use core_graphics::event::{CGEventType, EventField};
        let mut keyboard = KeyboardInjector::default();
        for (vk, mods, flag) in [
            (
                0x37,
                KeyModifiers::COMMAND,
                CGEventFlags::CGEventFlagCommand,
            ),
            (0x38, KeyModifiers::SHIFT, CGEventFlags::CGEventFlagShift),
            (
                0x3b,
                KeyModifiers::CONTROL,
                CGEventFlags::CGEventFlagControl,
            ),
            (
                0x3a,
                KeyModifiers::OPTION,
                CGEventFlags::CGEventFlagAlternate,
            ),
            (
                0x39,
                KeyModifiers::CAPS_LOCK,
                CGEventFlags::CGEventFlagAlphaShift,
            ),
            (
                0x3f,
                KeyModifiers::FUNCTION,
                CGEventFlags::CGEventFlagSecondaryFn,
            ),
        ] {
            let down = keyboard
                .build(vk, mods, KeyKind::FlagsChanged, None)
                .unwrap()
                .unwrap();
            assert_eq!(down.get_type() as u32, CGEventType::FlagsChanged as u32);
            assert!(down.get_flags().contains(flag));
            assert_eq!(keyboard.modifiers, mods);
            let up = keyboard
                .build(vk, KeyModifiers::empty(), KeyKind::FlagsChanged, None)
                .unwrap()
                .unwrap();
            assert_eq!(up.get_type() as u32, CGEventType::FlagsChanged as u32);
            assert!(!up.get_flags().contains(flag));
        }
        for (kind, repeat) in [
            (KeyKind::Down, 0),
            (KeyKind::Down, 1),
            (KeyKind::Up, 0),
            (KeyKind::Down, 0),
        ] {
            let event = keyboard
                .build(0, KeyModifiers::SHIFT, kind, Some('A'))
                .unwrap()
                .unwrap();
            assert_eq!(
                event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT),
                repeat
            );
            assert!(event.get_flags().contains(CGEventFlags::CGEventFlagShift));
        }
    }

    #[test]
    fn scroll_preserves_fractional_deltas_direction_phases_and_flags() {
        use core_graphics::event::CGEventType;
        for (phase, value) in [
            (ScrollPhase::Began, 1),
            (ScrollPhase::Changed, 2),
            (ScrollPhase::Ended, 4),
        ] {
            let event = make_scroll_event(1.25, -2.5, phase, KeyModifiers::CONTROL).unwrap();
            assert_eq!(event.get_type() as u32, CGEventType::ScrollWheel as u32);
            assert_eq!(
                event.get_double_value_field(SCROLL_WHEEL_EVENT_FIXED_PT_DELTA_AXIS_1),
                2.5
            );
            assert_eq!(
                event.get_double_value_field(SCROLL_WHEEL_EVENT_FIXED_PT_DELTA_AXIS_2),
                -1.25
            );
            assert_eq!(
                event.get_integer_value_field(SCROLL_WHEEL_EVENT_SCROLL_PHASE),
                value
            );
            assert!(event.get_flags().contains(CGEventFlags::CGEventFlagControl));
        }
    }

    #[test]
    fn control_keys_keep_physical_semantics_and_text_keeps_client_layout() {
        for (vk, text) in [
            (0x24, '\n'),
            (0x4c, '\r'),
            (0x30, '\t'),
            (0x35, '\u{1b}'),
            (0x7b, '\u{f702}'),
        ] {
            assert_eq!(key_text(vk, KeyModifiers::empty(), Some(text)), None);
        }
        assert_eq!(key_text(0, KeyModifiers::empty(), Some('é')), Some('é'));
        assert_eq!(key_text(0x31, KeyModifiers::empty(), Some(' ')), Some(' '));
        assert_eq!(key_text(0, KeyModifiers::COMMAND, Some('a')), None);
    }

    #[test]
    fn unicode_only_vk_is_no_modifier() {
        // The sentinel must never be mistaken for a modifier key (host release
        // tracking infers press/release direction from modifier_of_vk).
        assert_eq!(modifier_of_vk(UNICODE_ONLY_VK), None);
        assert_eq!(modifier_flag_of_vk(UNICODE_ONLY_VK), None);
    }

    #[test]
    fn unicode_only_vk_never_collides_with_table_keys() {
        // 'a' is the real vk 0x00 the sentinel replaces in the vk-0 fallback.
        assert_eq!(modifier_of_vk(0x00), None);
        assert_ne!(UNICODE_ONLY_VK, 0x00);
    }
}

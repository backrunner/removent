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

fn source() -> Result<core_graphics::event_source::CGEventSource, String> {
    core_graphics::event_source::CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|_| "event source create failed".to_string())
}

fn flags_of(mods: KeyModifiers) -> CGEventFlags {
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
    match kind {
        KeyKind::FlagsChanged => {
            // Modifier key's own state change: macOS FlagsChanged does not
            // distinguish down/up; infer the direction from whether the event's
            // flags have this modifier set.
            let flags = flags_of_flags_changed(modifiers);
            let down = modifier_flag_of_vk(vk_code)
                .map(|f| flags.intersects(f))
                .unwrap_or(true);
            let event = CGEvent::new_keyboard_event(source()?, vk_code, down)
                .map_err(|_| "flags event create failed".to_string())?;
            event.set_flags(flags);
            event.post(CGEventTapLocation::HID);
            Ok(())
        }
        KeyKind::Down | KeyKind::Up => {
            let down = kind == KeyKind::Down;
            // UNICODE_ONLY_VK carries no physical key; CGEvent cannot express a
            // keycode-less event, so post with vk 0 and let the unicode string
            // carry the character (same injection as the old vk-0 fallback).
            let vk = if vk_code == UNICODE_ONLY_VK {
                0
            } else {
                vk_code
            };
            let event = CGEvent::new_keyboard_event(source()?, vk, down)
                .map_err(|_| "keyboard event create failed".to_string())?;
            event.set_flags(flags_of(modifiers));
            if let Some(c) = unicode
                && !modifiers.intersects(KeyModifiers::COMMAND | KeyModifiers::CONTROL)
            {
                let mut buf = [0u16; 2];
                event.set_string_from_utf16_unchecked(c.encode_utf16(&mut buf));
            }
            event.post(CGEventTapLocation::HID);
            Ok(())
        }
    }
}

/// Construction check (not posted): verifies the event can be created.
pub fn build_key(vk_code: u16, modifiers: KeyModifiers, kind: KeyKind) -> Result<(), String> {
    let src = source()?;
    let down = matches!(kind, KeyKind::Down);
    let ev = CGEvent::new_keyboard_event(src, vk_code, down)
        .map_err(|_| "keyboard event create failed".to_string())?;
    ev.set_flags(flags_of(modifiers));
    Ok(())
}

/// `kCGScrollWheelEventScrollPhase` (not exported by core-graphics).
const SCROLL_WHEEL_EVENT_SCROLL_PHASE: core_graphics::event::CGEventField = 99;

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
/// being passed in. PIXEL units + double-precision pointDelta preserve
/// sub-pixel precision; `phase` passes the scroll phase through so the receiver
/// can synthesize momentum/elastic scrolling.
pub fn inject_scroll_pixels(dx_px: f64, dy_px: f64, phase: ScrollPhase) -> Result<(), String> {
    let src = source()?;
    let event = CGEvent::new_scroll_event(
        src,
        core_graphics::event::ScrollEventUnit::PIXEL,
        2,
        -(dy_px.round() as i32),
        -(dx_px.round() as i32),
        0,
    )
    .map_err(|_| "scroll event create failed".to_string())?;
    use core_graphics::event::EventField;
    event.set_double_value_field(EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_1, -dy_px);
    event.set_double_value_field(EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_2, -dx_px);
    event.set_integer_value_field(EventField::SCROLL_WHEEL_EVENT_IS_CONTINUOUS, 1);
    event.set_integer_value_field(SCROLL_WHEEL_EVENT_SCROLL_PHASE, cg_scroll_phase(phase));
    event.post(CGEventTapLocation::HID);
    Ok(())
}

/// Construction check (not posted).
pub fn build_scroll(dx_px: f64, dy_px: f64, phase: ScrollPhase) -> Result<(), String> {
    let src = source()?;
    let ev = CGEvent::new_scroll_event(
        src,
        core_graphics::event::ScrollEventUnit::PIXEL,
        2,
        -(dy_px.round() as i32),
        -(dx_px.round() as i32),
        0,
    )
    .map_err(|_| "scroll event create failed".to_string())?;
    use core_graphics::event::EventField;
    ev.set_double_value_field(EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_1, -dy_px);
    ev.set_double_value_field(EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_2, -dx_px);
    ev.set_integer_value_field(EventField::SCROLL_WHEEL_EVENT_IS_CONTINUOUS, 1);
    ev.set_integer_value_field(SCROLL_WHEEL_EVENT_SCROLL_PHASE, cg_scroll_phase(phase));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

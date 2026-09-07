use ironrdp::input::{Database, MouseButton, MousePosition, Operation, Scancode, WheelRotations};
use ironrdp::pdu::input::fast_path::FastPathInputEvent;
use removent_proto::{ControlMsg, KeyKind, KeyModifiers};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct InputState {
    database: Database,
    unicode_keys: HashMap<u16, char>,
    caps_lock: Option<bool>,
}

impl InputState {
    pub(super) fn translate(
        &mut self,
        command: ControlMsg,
        width: u16,
        height: u16,
    ) -> Vec<FastPathInputEvent> {
        let mut ops = Vec::new();
        match command {
            ControlMsg::MouseEvent {
                x_px,
                y_px,
                buttons,
                ..
            } => {
                ops.push(Operation::MouseMove(MousePosition {
                    x: x_px.clamp(0., f32::from(width.saturating_sub(1))) as u16,
                    y: y_px.clamp(0., f32::from(height.saturating_sub(1))) as u16,
                }));
                for (mask, button) in [
                    (1, MouseButton::Left),
                    (2, MouseButton::Right),
                    (4, MouseButton::Middle),
                ] {
                    ops.push(if buttons & mask != 0 {
                        Operation::MouseButtonPressed(button)
                    } else {
                        Operation::MouseButtonReleased(button)
                    });
                }
            }
            ControlMsg::ScrollEvent { dx_mm, dy_mm, .. } => {
                // Wire Y is positive down; RDP vertical wheel units are positive up.
                // Horizontal wheel units, like wire X, are positive right.
                for (is_vertical, delta) in [(true, -dy_mm), (false, dx_mm)] {
                    if delta.is_finite() && delta != 0. {
                        ops.push(Operation::WheelRotations(WheelRotations {
                            is_vertical,
                            rotation_units: (delta * 40.).clamp(-255., 255.) as i16,
                        }));
                    }
                }
            }
            ControlMsg::KeyEvent {
                vk_code,
                modifiers,
                kind,
                unicode,
            } => {
                for (mask, code) in [
                    (KeyModifiers::SHIFT, 0x2a),
                    (KeyModifiers::CONTROL, 0x1d),
                    (KeyModifiers::OPTION, 0x38),
                    (KeyModifiers::COMMAND, 0xe05b),
                ] {
                    let scan = Scancode::from_u16(code);
                    let pressed = modifiers.contains(mask);
                    if self.database.is_key_pressed(scan) != pressed {
                        ops.push(if pressed {
                            Operation::KeyPressed(scan)
                        } else {
                            Operation::KeyReleased(scan)
                        });
                    }
                }
                let modifier = matches!(vk_code, 0x36..=0x3f);
                if !modifier {
                    if matches!(kind, KeyKind::Up) {
                        if let Some(ch) = self.unicode_keys.remove(&vk_code) {
                            ops.push(Operation::UnicodeKeyReleased(ch));
                        } else if let Some(scan) = mac_scancode(vk_code) {
                            ops.push(Operation::KeyReleased(scan));
                        }
                    } else if matches!(kind, KeyKind::Down) {
                        let shortcut =
                            modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::COMMAND);
                        let scan = mac_scancode(vk_code);
                        let text = unicode.filter(|c| {
                            !shortcut
                                && (!c.is_ascii() || scan.is_none())
                                && !scan.is_some_and(|scan| self.database.is_key_pressed(scan))
                        });
                        if let Some(ch) = self.unicode_keys.get(&vk_code).copied().or(text) {
                            ops.push(Operation::UnicodeKeyPressed(ch));
                            if vk_code == u16::MAX {
                                // The viewer's Unicode-only sentinel identifies text,
                                // not a physical key. Its key-up may have no key_char;
                                // finish each character now so overlapping text does
                                // not repeat the first character or leave it pressed.
                                ops.push(Operation::UnicodeKeyReleased(ch));
                            } else {
                                self.unicode_keys.insert(vk_code, ch);
                            }
                        } else if let Some(scan) = scan {
                            ops.push(Operation::KeyPressed(scan));
                        }
                    }
                }
                // Synchronize before the first key that depends on the lock state.
                let caps_lock = modifiers.contains(KeyModifiers::CAPS_LOCK);
                let mut events = Vec::new();
                if self.caps_lock != Some(caps_lock) {
                    self.caps_lock = Some(caps_lock);
                    events.push(ironrdp::input::synchronize_event(
                        false, true, caps_lock, false,
                    ));
                }
                events.extend(self.database.apply(ops));
                return events;
            }
            _ => {}
        }
        self.database.apply(ops).into_vec()
    }
}

/// macOS virtual keys to IBM PC set-1 scancodes, including the E0 prefix.
fn mac_scancode(vk: u16) -> Option<Scancode> {
    let code = match vk {
        0x00 => 0x1e,
        0x01 => 0x1f,
        0x02 => 0x20,
        0x03 => 0x21,
        0x04 => 0x23,
        0x05 => 0x22,
        0x06 => 0x2c,
        0x07 => 0x2d,
        0x08 => 0x2e,
        0x09 => 0x2f,
        0x0b => 0x30,
        0x0c => 0x10,
        0x0d => 0x11,
        0x0e => 0x12,
        0x0f => 0x13,
        0x10 => 0x15,
        0x11 => 0x14,
        0x12 => 0x02,
        0x13 => 0x03,
        0x14 => 0x04,
        0x15 => 0x05,
        0x16 => 0x07,
        0x17 => 0x06,
        0x18 => 0x0d,
        0x19 => 0x0a,
        0x1a => 0x08,
        0x1b => 0x0c,
        0x1c => 0x09,
        0x1d => 0x0b,
        0x1e => 0x1b,
        0x1f => 0x18,
        0x20 => 0x16,
        0x21 => 0x1a,
        0x22 => 0x17,
        0x23 => 0x19,
        0x24 => 0x1c,
        0x25 => 0x26,
        0x26 => 0x24,
        0x27 => 0x28,
        0x28 => 0x25,
        0x29 => 0x27,
        0x2a => 0x2b,
        0x2b => 0x33,
        0x2c => 0x35,
        0x2d => 0x31,
        0x2e => 0x32,
        0x2f => 0x34,
        0x30 => 0x0f,
        0x31 => 0x39,
        0x32 => 0x29,
        0x33 => 0x0e,
        0x35 => 0x01,
        0x41 => 0x53,
        0x43 => 0x37,
        0x45 => 0x4e,
        0x47 => 0x45,
        0x4b => 0xe035,
        0x4c => 0xe01c,
        0x4e => 0x4a,
        0x52 => 0x52,
        0x53 => 0x4f,
        0x54 => 0x50,
        0x55 => 0x51,
        0x56 => 0x4b,
        0x57 => 0x4c,
        0x58 => 0x4d,
        0x59 => 0x47,
        0x5b => 0x48,
        0x5c => 0x49,
        0x60 => 0x3f,
        0x61 => 0x40,
        0x62 => 0x41,
        0x63 => 0x3d,
        0x64 => 0x42,
        0x65 => 0x43,
        0x67 => 0x57,
        0x6d => 0x44,
        0x6f => 0x58,
        0x72 => 0xe052,
        0x73 => 0xe047,
        0x74 => 0xe049,
        0x75 => 0xe053,
        0x76 => 0x3e,
        0x77 => 0xe04f,
        0x78 => 0x3c,
        0x79 => 0xe051,
        0x7a => 0x3b,
        0x7b => 0xe04b,
        0x7c => 0xe04d,
        0x7d => 0xe050,
        0x7e => 0xe048,
        _ => return None,
    };
    Some(Scancode::from_u16(code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironrdp::pdu::input::{fast_path::KeyboardFlags, mouse::PointerFlags};

    #[test]
    fn extended_keys_and_shortcuts() {
        let mut state = InputState::default();
        let events = state.translate(
            ControlMsg::KeyEvent {
                vk_code: 0x7b,
                modifiers: KeyModifiers::CONTROL,
                kind: KeyKind::Down,
                unicode: None,
            },
            100,
            100,
        );
        assert!(
            matches!(events[1], FastPathInputEvent::KeyboardEvent(flags, 0x1d) if flags.is_empty())
        );
        assert!(
            matches!(events[2], FastPathInputEvent::KeyboardEvent(flags, 0x4b) if flags == KeyboardFlags::EXTENDED)
        );
    }

    #[test]
    fn unicode_release_survives_missing_key_char() {
        let mut state = InputState::default();
        state.translate(
            ControlMsg::KeyEvent {
                vk_code: 0x0e,
                modifiers: KeyModifiers::empty(),
                kind: KeyKind::Down,
                unicode: Some('\u{4e2d}'),
            },
            100,
            100,
        );
        let events = state.translate(
            ControlMsg::KeyEvent {
                vk_code: 0x0e,
                modifiers: KeyModifiers::empty(),
                kind: KeyKind::Up,
                unicode: None,
            },
            100,
            100,
        );
        assert!(
            matches!(events[0], FastPathInputEvent::UnicodeKeyboardEvent(flags, 0x4e2d) if flags == KeyboardFlags::RELEASE)
        );
    }

    #[test]
    fn right_button_and_release_use_clamped_position() {
        let mut state = InputState::default();
        let command = |buttons| ControlMsg::MouseEvent {
            display_id: 0,
            x_px: 150.,
            y_px: -10.,
            buttons,
            kind: removent_proto::MouseKind::Moved,
        };
        let events = state.translate(command(2), 100, 100);
        assert!(events.iter().any(|e| matches!(e, FastPathInputEvent::MouseEvent(m) if m.flags == PointerFlags::RIGHT_BUTTON | PointerFlags::DOWN && m.x_position == 99 && m.y_position == 0)));
        let events = state.translate(command(0), 100, 100);
        assert!(
            matches!(&events[0], FastPathInputEvent::MouseEvent(m) if m.flags == PointerFlags::RIGHT_BUTTON)
        );
    }

    #[test]
    fn scroll_directions_match_wire_coordinates() {
        let mut state = InputState::default();
        let events = state.translate(
            ControlMsg::ScrollEvent {
                display_id: 0,
                dx_mm: 3.,
                dy_mm: -3.,
                phase: removent_proto::ScrollPhase::Changed,
            },
            100,
            100,
        );
        assert!(
            matches!(&events[0], FastPathInputEvent::MouseEvent(m) if m.flags == PointerFlags::VERTICAL_WHEEL && m.number_of_wheel_rotation_units == 120)
        );
        assert!(
            matches!(&events[1], FastPathInputEvent::MouseEvent(m) if m.flags == PointerFlags::HORIZONTAL_WHEEL && m.number_of_wheel_rotation_units == 120)
        );
    }

    #[test]
    fn unicode_only_text_without_key_up_does_not_repeat_the_previous_character() {
        let mut state = InputState::default();
        for ch in ['é', '中'] {
            let events = state.translate(
                ControlMsg::KeyEvent {
                    vk_code: u16::MAX,
                    modifiers: KeyModifiers::empty(),
                    kind: KeyKind::Down,
                    unicode: Some(ch),
                },
                100,
                100,
            );
            assert!(events.iter().any(|event| matches!(event, FastPathInputEvent::UnicodeKeyboardEvent(flags, code) if flags.is_empty() && u32::from(*code) == ch as u32)));
            assert!(events.iter().any(|event| matches!(event, FastPathInputEvent::UnicodeKeyboardEvent(flags, code) if *flags == KeyboardFlags::RELEASE && u32::from(*code) == ch as u32)));
        }
        assert!(state.unicode_keys.is_empty());
    }

    #[test]
    fn changing_text_during_key_repeat_still_releases_the_original_scancode() {
        let mut state = InputState::default();
        for ch in ['e', 'é'] {
            state.translate(
                ControlMsg::KeyEvent {
                    vk_code: 0x0e,
                    modifiers: KeyModifiers::empty(),
                    kind: KeyKind::Down,
                    unicode: Some(ch),
                },
                100,
                100,
            );
        }
        let events = state.translate(
            ControlMsg::KeyEvent {
                vk_code: 0x0e,
                modifiers: KeyModifiers::empty(),
                kind: KeyKind::Up,
                unicode: None,
            },
            100,
            100,
        );
        assert!(
            matches!(events[0], FastPathInputEvent::KeyboardEvent(flags, 0x12) if flags == KeyboardFlags::RELEASE)
        );
        assert!(!state.database.is_key_pressed(Scancode::from_u16(0x12)));
    }

    #[test]
    fn caps_lock_is_synchronized_before_typing_and_shortcuts_use_scancodes() {
        let mut state = InputState::default();
        let modifiers = KeyModifiers::CAPS_LOCK | KeyModifiers::CONTROL;
        let events = state.translate(
            ControlMsg::KeyEvent {
                vk_code: 0x0e,
                modifiers,
                kind: KeyKind::Down,
                unicode: Some('é'),
            },
            100,
            100,
        );
        assert_eq!(
            events[0],
            ironrdp::input::synchronize_event(false, true, true, false)
        );
        assert!(
            matches!(events[2], FastPathInputEvent::KeyboardEvent(flags, 0x12) if flags.is_empty())
        );
        let events = state.translate(
            ControlMsg::KeyEvent {
                vk_code: 0x0e,
                modifiers,
                kind: KeyKind::Up,
                unicode: None,
            },
            100,
            100,
        );
        assert_eq!(events.len(), 1);
        assert!(
            matches!(events[0], FastPathInputEvent::KeyboardEvent(flags, 0x12) if flags == KeyboardFlags::RELEASE)
        );
    }
}

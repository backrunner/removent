//! Bounded, content-free physical held-state. No text, position, or history.
pub struct EventMetadata {
    pub kind: u32,
    pub key: u16,
    pub button: i64,
    pub user_data: i64,
    pub process_id: i64,
    pub physical_key_down: bool,
}
pub fn decode(event: EventMetadata) -> Option<LocalEvent> {
    if removent_input::origin::is_our_source(event.user_data, event.process_id) {
        return None;
    }
    Some(match event.kind {
        10 | 11 => LocalEvent::Key {
            code: event.key,
            down: event.kind == 10,
        },
        12 => LocalEvent::Key {
            code: event.key,
            down: event.physical_key_down,
        },
        1 | 2 | 3 | 4 | 6 | 7 | 25 | 26 | 27 => LocalEvent::Button {
            index: match event.kind {
                1 | 2 | 6 => 0,
                3 | 4 | 7 => 1,
                _ => u8::try_from(event.button).unwrap_or(u8::MAX),
            },
            down: !matches!(event.kind, 2 | 4 | 26),
        },
        5 | 22 => LocalEvent::Activity,
        _ => return None,
    })
}
pub enum LocalEvent {
    Key { code: u16, down: bool },
    Button { index: u8, down: bool },
    Activity,
}
pub type InputState = crate::control::LocalHeld;
impl InputState {
    pub fn apply(&mut self, event: LocalEvent) {
        match event {
            LocalEvent::Key { code, down } => {
                self.set_key(code, down);
            }
            LocalEvent::Button { index, down } => {
                self.set_button(index, down);
            }
            LocalEvent::Activity => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_graphics::{
        event::{CGEvent, EventField},
        event_source::{CGEventSource, CGEventSourceStateID},
    };
    fn event(kind: u32, key: u16, button: i64, down: bool) -> EventMetadata {
        EventMetadata {
            kind,
            key,
            button,
            physical_key_down: down,
            user_data: 0,
            process_id: 0,
        }
    }
    #[test]
    fn synthetic_events_and_their_cleanup_do_not_pause_or_release_physical_input() {
        let synthetic = CGEvent::new_keyboard_event(
            CGEventSource::new(CGEventSourceStateID::CombinedSessionState).unwrap(),
            0,
            false,
        )
        .unwrap();
        removent_input::origin::mark(&synthetic);
        assert!(removent_input::origin::is_our_event(&synthetic));
        let mut state = InputState::default();
        state.apply(decode(event(10, 0, 0, false)).unwrap());
        for kind in super::super::EVENTS {
            let metadata = EventMetadata {
                user_data: synthetic.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA),
                process_id: synthetic
                    .get_integer_value_field(EventField::EVENT_SOURCE_UNIX_PROCESS_ID),
                ..event(*kind, 0, 0, false)
            };
            assert!(decode(metadata).is_none());
        }
        assert!(state.held());
        state.apply(decode(event(11, 0, 0, false)).unwrap());
        assert!(!state.held());
    }
    #[test]
    fn modifiers_buttons_and_caps_lock_have_independent_held_state() {
        let mut state = InputState::default();
        state.apply(decode(event(12, 0x39, 0, true)).unwrap());
        assert!(!state.held());
        for key in [0x38, 0x3c] {
            state.apply(decode(event(12, key, 0, true)).unwrap());
        }
        state.apply(decode(event(12, 0x38, 0, false)).unwrap());
        assert!(state.held());
        state.apply(decode(event(25, 0, 31, false)).unwrap());
        state.apply(decode(event(12, 0x3c, 0, false)).unwrap());
        assert!(state.held());
        state.apply(decode(event(5, 0, 0, false)).unwrap());
        state.apply(decode(event(22, 0, 0, false)).unwrap());
        assert!(state.held());
        state.apply(decode(event(26, 0, 31, false)).unwrap());
        assert!(!state.held());
        for metadata in [
            event(10, u16::MAX, 0, true),
            event(25, 0, -1, true),
            event(25, 0, 400, true),
        ] {
            state.apply(decode(metadata).unwrap());
        }
        assert!(!state.held());
    }
    #[test]
    fn motion_scroll_and_external_automation_are_activity_without_recording_content() {
        for kind in [5, 22] {
            assert!(matches!(
                decode(event(kind, 0, 0, false)),
                Some(LocalEvent::Activity)
            ));
        }
        let mut external = event(10, 0, 0, true);
        external.process_id = i64::from(std::process::id()) + 1;
        assert!(matches!(
            decode(external),
            Some(LocalEvent::Key { down: true, .. })
        ));
        assert!(decode(event(0, 0, 0, false)).is_none());
    }
}

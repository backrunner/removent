//! Marks our synthetic events so the host's local-input monitor cannot feed back.
use core_graphics::event::{CGEvent, EventField};
use std::sync::OnceLock;

fn marker() -> i64 {
    static MARKER: OnceLock<i64> = OnceLock::new();
    *MARKER.get_or_init(|| rand::random::<i64>() | 1)
}

/// Initialize before installing an event-tap callback, keeping OS entropy
/// access off the event-delivery path.
pub fn prepare() {
    let _ = marker();
}

pub fn mark(event: &CGEvent) {
    event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, marker());
}

/// An origin hint, not an authentication mechanism. Other local automation is
/// treated as local input too; only this process's marked events are excluded.
pub fn is_our_event(event: &CGEvent) -> bool {
    is_our_source(
        event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA),
        event.get_integer_value_field(EventField::EVENT_SOURCE_UNIX_PROCESS_ID),
    )
}

pub fn is_our_source(user_data: i64, process_id: i64) -> bool {
    user_data == marker() && process_id == i64::from(std::process::id())
}

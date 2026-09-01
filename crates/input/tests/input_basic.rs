//! Real clipboard roundtrip (runnable headless); event construction path validation.

use removent_input::{build_key, change_count, read_text, write_text};
use removent_proto::{KeyKind, KeyModifiers};

#[test]
fn clipboard_text_roundtrip() {
    let marker = format!("removent-clip-{}", std::process::id());
    write_text(&marker).expect("write");
    assert_eq!(read_text().expect("read"), marker);
    // changeCount must have incremented after the write: two reads should be stable.
    let c1 = change_count().unwrap();
    let c2 = change_count().unwrap();
    assert_eq!(c1, c2);
}

#[test]
fn key_event_construction() {
    // Not posted; only verifies construction and modifier mapping don't panic.
    build_key(0x00 /* A */, KeyModifiers::empty(), KeyKind::Down).expect("plain down");
    build_key(
        0x2C, /* / */
        KeyModifiers::COMMAND | KeyModifiers::SHIFT,
        KeyKind::Up,
    )
    .expect("combo up");
}

#[test]
fn display_geometry_present() {
    let displays = removent_input::display_list();
    assert!(
        !displays.is_empty(),
        "at least one display required in test env"
    );
    assert!(displays.iter().any(|d| d.is_main));
    assert!(displays[0].w_px > 0 && displays[0].h_px > 0);
}

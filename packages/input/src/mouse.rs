//! Mouse event injection (CGEventPost → HID session).

use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton};
use removent_proto::MouseKind;

/// Injects a mouse event at global display coordinates (points, the coordinate
/// space CGEvent expects; the caller converts capture pixels → points first).
pub fn inject_mouse(kind: MouseKind, x_pt: f64, y_pt: f64) -> Result<(), String> {
    let event = make_mouse_event(kind, x_pt, y_pt)?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

fn make_mouse_event(kind: MouseKind, x_pt: f64, y_pt: f64) -> Result<CGEvent, String> {
    let (cg_type, button) = match kind {
        MouseKind::Moved => (CGEventType::MouseMoved, None),
        MouseKind::LeftDown => (CGEventType::LeftMouseDown, Some(CGMouseButton::Left)),
        MouseKind::LeftUp => (CGEventType::LeftMouseUp, Some(CGMouseButton::Left)),
        MouseKind::RightDown => (CGEventType::RightMouseDown, Some(CGMouseButton::Right)),
        MouseKind::RightUp => (CGEventType::RightMouseUp, Some(CGMouseButton::Right)),
        MouseKind::MiddleDown => (CGEventType::OtherMouseDown, Some(CGMouseButton::Center)),
        MouseKind::MiddleUp => (CGEventType::OtherMouseUp, Some(CGMouseButton::Center)),
        MouseKind::LeftDragged => (CGEventType::LeftMouseDragged, Some(CGMouseButton::Left)),
        MouseKind::RightDragged => (CGEventType::RightMouseDragged, Some(CGMouseButton::Right)),
        MouseKind::MiddleDragged => (CGEventType::OtherMouseDragged, Some(CGMouseButton::Center)),
    };
    let source = core_graphics::event_source::CGEventSource::new(
        core_graphics::event_source::CGEventSourceStateID::CombinedSessionState,
    )
    .map_err(|_| "event source create failed".to_string())?;
    CGEvent::new_mouse_event(
        source,
        cg_type,
        cg_point(x_pt, y_pt),
        button.unwrap_or(CGMouseButton::Left),
    )
    .map_err(|_| "mouse event create failed".to_string())
}

fn cg_point(x: f64, y: f64) -> core_graphics::geometry::CGPoint {
    core_graphics::geometry::CGPoint { x, y }
}

/// Builds without posting (validates the event construction path in
/// permission-less environments).
pub fn build_mouse_event(kind: MouseKind, x_px: f64, y_px: f64) -> Result<(), String> {
    make_mouse_event(kind, x_px, y_px).map(|_| ())
}

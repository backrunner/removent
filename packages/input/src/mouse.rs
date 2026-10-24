//! Mouse event injection (CGEventPost → HID session).

use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField,
};
use removent_proto::{KeyModifiers, MouseKind};
use std::time::{Duration, Instant};

const CLICK_SLOP_PT: f64 = 4.0;

#[derive(Clone, Copy)]
struct Click {
    button: usize,
    at: Instant,
    x: f64,
    y: f64,
    count: i64,
}

/// Each remote session owns its click history. CGEventCreateMouseEvent does
/// not infer a double click from successive synthetic down/up events.
#[derive(Default)]
pub struct MouseInjector {
    last_click: Option<Click>,
    held: [Option<(i64, i64)>; 3], // (click count, matching down/up event number)
    event_number: i64,
}

impl MouseInjector {
    pub fn inject(
        &mut self,
        kind: MouseKind,
        x: f64,
        y: f64,
        modifiers: KeyModifiers,
    ) -> Result<(), String> {
        let interval = Duration::from_secs_f64(objc2_app_kit::NSEvent::doubleClickInterval());
        self.build(
            kind,
            x,
            y,
            crate::keyboard::flags_of(modifiers),
            Instant::now(),
            interval,
        )?
        .post(CGEventTapLocation::HID);
        Ok(())
    }

    fn build(
        &mut self,
        kind: MouseKind,
        x: f64,
        y: f64,
        flags: CGEventFlags,
        now: Instant,
        interval: Duration,
    ) -> Result<CGEvent, String> {
        let event = make_mouse_event(kind, x, y)?;
        event.set_flags(flags);
        let button = match kind {
            MouseKind::LeftDown | MouseKind::LeftUp | MouseKind::LeftDragged => 0,
            MouseKind::RightDown | MouseKind::RightUp | MouseKind::RightDragged => 1,
            MouseKind::MiddleDown | MouseKind::MiddleUp | MouseKind::MiddleDragged => 2,
            MouseKind::Moved => {
                self.invalidate_after_motion(x, y);
                return Ok(event);
            }
        };
        let down = matches!(
            kind,
            MouseKind::LeftDown | MouseKind::RightDown | MouseKind::MiddleDown
        );
        let up = matches!(
            kind,
            MouseKind::LeftUp | MouseKind::RightUp | MouseKind::MiddleUp
        );
        let (count, number) = if down {
            let count = self
                .last_click
                .filter(|c| {
                    c.button == button
                        && self.held[button].is_none()
                        && now.saturating_duration_since(c.at) <= interval
                        && (x - c.x).abs() <= CLICK_SLOP_PT
                        && (y - c.y).abs() <= CLICK_SLOP_PT
                })
                .map_or(1, |c| c.count.saturating_add(1));
            self.event_number = self.event_number.saturating_add(1);
            self.last_click = Some(Click {
                button,
                at: now,
                x,
                y,
                count,
            });
            self.held[button] = Some((count, self.event_number));
            (count, self.event_number)
        } else {
            let pair = self.held[button].unwrap_or((1, self.event_number));
            self.invalidate_after_motion(x, y);
            if up {
                self.held[button] = None;
            }
            pair
        };
        event.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, count);
        event.set_integer_value_field(EventField::MOUSE_EVENT_NUMBER, number);
        Ok(event)
    }

    fn invalidate_after_motion(&mut self, x: f64, y: f64) {
        if self
            .last_click
            .is_some_and(|c| (x - c.x).abs() > CLICK_SLOP_PT || (y - c.y).abs() > CLICK_SLOP_PT)
        {
            self.last_click = None;
        }
    }
}

/// Injects a mouse event at global display coordinates (points, the coordinate
/// space CGEvent expects; the caller converts capture pixels → points first).
pub fn inject_mouse(kind: MouseKind, x_pt: f64, y_pt: f64) -> Result<(), String> {
    // Convenience API for a single local producer. Hosts use one MouseInjector
    // per session so peers never share click history.
    static INJECTOR: std::sync::Mutex<MouseInjector> = std::sync::Mutex::new(MouseInjector {
        last_click: None,
        held: [None; 3],
        event_number: 0,
    });
    INJECTOR
        .lock()
        .unwrap()
        .inject(kind, x_pt, y_pt, KeyModifiers::empty())
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

#[cfg(test)]
mod tests;

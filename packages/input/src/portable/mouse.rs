#[cfg(not(windows))]
use enigo::Coordinate;
use enigo::{Button, Direction, Mouse};
use removent_proto::{KeyModifiers, MouseKind};
#[derive(Default)]
pub struct MouseInjector;
impl MouseInjector {
    pub fn inject(
        &mut self,
        kind: MouseKind,
        x: f64,
        y: f64,
        _mods: KeyModifiers,
    ) -> Result<(), String> {
        // Enigo's Windows absolute motion normalizes against the primary
        // monitor only. Physical coordinates also cover negative monitor
        // origins and avoid DPI virtualization on secondary displays.
        #[cfg(windows)]
        if unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetPhysicalCursorPos(
                x.round() as i32,
                y.round() as i32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        crate::portable::with(|e| {
            #[cfg(not(windows))]
            e.move_mouse(x.round() as i32, y.round() as i32, Coordinate::Abs)?;
            let (button, direction) = match kind {
                MouseKind::LeftDown => (Button::Left, Direction::Press),
                MouseKind::LeftUp => (Button::Left, Direction::Release),
                MouseKind::RightDown => (Button::Right, Direction::Press),
                MouseKind::RightUp => (Button::Right, Direction::Release),
                MouseKind::MiddleDown => (Button::Middle, Direction::Press),
                MouseKind::MiddleUp => (Button::Middle, Direction::Release),
                _ => return Ok(()),
            };
            e.button(button, direction)
        })
    }
}
pub fn inject_mouse(kind: MouseKind, x: f64, y: f64) -> Result<(), String> {
    MouseInjector.inject(kind, x, y, KeyModifiers::empty())
}

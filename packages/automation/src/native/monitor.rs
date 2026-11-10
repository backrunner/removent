//! Passive, session-scoped local-input observation. No text or coordinates leave
//! this thread; the executor only receives held-state and activity notifications.
use crate::control::Control;
use core_foundation::{
    base::TCFType,
    mach_port::{CFMachPort, CFMachPortRef},
    runloop::{CFRunLoop, kCFRunLoopDefaultMode},
};
use std::{ffi::c_void, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

mod events;
mod registration;
use events::{EventMetadata, InputState, LocalEvent};

const POLL: Duration = Duration::from_millis(100);
const EVENTS: &[u32] = &[1, 2, 3, 4, 5, 6, 7, 10, 11, 12, 22, 25, 26, 27];
const HID_STATE: i32 = 1;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGEventTapCreate(
        location: u32,
        placement: u32,
        options: u32,
        mask: u64,
        callback: unsafe extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void) -> *mut c_void,
        user: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventTapIsEnabled(tap: CFMachPortRef) -> bool;
    fn CGEventGetIntegerValueField(event: *mut c_void, field: u32) -> i64;
    fn CGEventSourceKeyState(source: i32, key: u16) -> bool;
    fn CGEventSourceButtonState(source: i32, button: u32) -> bool;
}
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn IsSecureEventInputEnabled() -> u8;
}

struct Tap(CFMachPort);
impl Drop for Tap {
    fn drop(&mut self) {
        // Invalidate before the callback context is destroyed, including unwind paths.
        unsafe {
            core_foundation::mach_port::CFMachPortInvalidate(self.0.as_concrete_TypeRef());
        }
    }
}

pub struct Monitor {
    stop: CancellationToken,
    watchdog: tokio::task::JoinHandle<()>,
}
impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop.cancel();
        self.watchdog.abort();
    }
}

pub fn start(
    control: Control,
    session: CancellationToken,
    runtime: &tokio::runtime::Handle,
    permit: Arc<tokio::sync::OwnedSemaphorePermit>,
) -> Monitor {
    let stop = session.child_token();
    let thread_stop = stop.clone();
    let observer = control.clone();
    // Retain the native slot until BOTH the native executor and this listener
    // have stopped. A slow OS call must not accumulate listener threads.
    if std::thread::Builder::new()
        .name("removent-local-input".into())
        .spawn(move || {
            let _permit = permit;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(&observer, &thread_stop)
            }));
            let reason = match result {
                Ok(Err(reason)) => reason,
                Ok(Ok(())) => "stopped",
                Err(_) => "monitor_failed",
            };
            observer.monitor_failed(reason);
            thread_stop.cancel();
        })
        .is_err()
    {
        control.monitor_failed("thread_unavailable");
    }
    let watch_stop = stop.clone();
    let watchdog = runtime.spawn(async move {
        loop {
            tokio::select! {
                _ = watch_stop.cancelled() => break,
                _ = tokio::time::sleep(POLL) => {
                    if control.expire_monitor() { watch_stop.cancel(); break; }
                },
            }
        }
    });
    Monitor { stop, watchdog }
}

fn availability() -> Result<(), &'static str> {
    // These preflight calls never request TCC permission. Carbon's secure-input
    // query is confined to this one listener thread, not the action worker.
    if !unsafe { CGPreflightListenEventAccess() } {
        return Err("input_monitoring_permission");
    }
    if unsafe { IsSecureEventInputEnabled() } != 0 {
        return Err("secure_input");
    }
    Ok(())
}

struct Observer {
    input: InputState,
    control: Control,
    stop: CancellationToken,
}

fn run(control: &Control, stop: &CancellationToken) -> Result<(), &'static str> {
    if stop.is_cancelled() {
        return Ok(());
    }
    availability()?;
    removent_input::origin::prepare();
    let mut observer = Box::new(Observer {
        input: InputState::default(),
        control: control.clone(),
        stop: stop.clone(),
    });
    let mask = EVENTS.iter().fold(0u64, |mask, kind| mask | (1u64 << kind));
    let before = registration::snapshot()?;
    // Session / head / listen-only: never intercept or delay local input.
    let raw = unsafe {
        CGEventTapCreate(
            1,
            0,
            1,
            mask,
            callback,
            (&mut *observer as *mut Observer).cast(),
        )
    };
    if raw.is_null() {
        return Err("tap_unavailable");
    }
    let tap = Tap(unsafe { CFMachPort::wrap_under_create_rule(raw) });
    let source = tap
        .0
        .create_runloop_source(0)
        .map_err(|_| "runloop_unavailable")?;
    let runloop = CFRunLoop::get_current();
    runloop.add_source(&source, unsafe { kCFRunLoopDefaultMode });
    unsafe {
        CGEventTapEnable(raw, true);
    }
    if !unsafe { CGEventTapIsEnabled(raw) } {
        return Err("tap_disabled");
    }
    registration::verify(&before, &registration::snapshot()?, mask)?;
    // Include input already held when the session starts. Sample only physical
    // HID state, before this session can dispatch any automation.
    for key in 0..128u16 {
        observer.input.apply(LocalEvent::Key {
            code: key,
            down: unsafe { CGEventSourceKeyState(HID_STATE, key) },
        });
    }
    for button in 0..32u8 {
        observer.input.apply(LocalEvent::Button {
            index: button,
            down: unsafe { CGEventSourceButtonState(HID_STATE, button.into()) },
        });
    }
    // Drain startup events before allowing mutations, so a down/up racing the
    // initial sample cannot be overlooked. Keep startup activity paused.
    CFRunLoop::run_in_mode(
        unsafe { kCFRunLoopDefaultMode },
        Duration::from_millis(1),
        false,
    );
    if stop.is_cancelled() {
        return Err("tap_disabled");
    }
    control.monitor_started(observer.input.held());
    while !stop.is_cancelled() {
        availability()?;
        if !unsafe { CGEventTapIsEnabled(raw) } {
            return Err("tap_disabled");
        }
        if !control.monitor_heartbeat() {
            return Err("monitor_unresponsive");
        }
        CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, POLL, false);
    }
    runloop.remove_source(&source, unsafe { kCFRunLoopDefaultMode });
    Ok(())
}

unsafe extern "C" fn callback(
    _proxy: *mut c_void,
    kind: u32,
    event: *mut c_void,
    context: *mut c_void,
) -> *mut c_void {
    // Disabled notifications can contain a NULL event. Do not construct a
    // CGEvent wrapper or read fields until the event pointer is validated.
    let observer = unsafe { &mut *context.cast::<Observer>() };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if kind == u32::MAX || kind == u32::MAX - 1 {
            observer.control.monitor_failed("tap_disabled");
            observer.stop.cancel();
            return;
        }
        if event.is_null() || observer.stop.is_cancelled() {
            return;
        }
        use core_graphics::event::EventField;
        let field = |field| unsafe { CGEventGetIntegerValueField(event, field) };
        let key = field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
        let Some(input) = events::decode(EventMetadata {
            kind,
            key,
            button: field(EventField::MOUSE_EVENT_BUTTON_NUMBER),
            user_data: field(EventField::EVENT_SOURCE_USER_DATA),
            process_id: field(EventField::EVENT_SOURCE_UNIX_PROCESS_ID),
            // Do not infer physical modifiers from aggregate flags, which may
            // include an agent-held modifier or the other Shift key.
            physical_key_down: kind == 12 && unsafe { CGEventSourceKeyState(HID_STATE, key) },
        }) else {
            return;
        };
        observer.input.apply(input);
        observer.control.observe_local(observer.input);
    }));
    if result.is_err() {
        observer.stop.cancel();
    }
    event
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn null_disabled_notifications_revoke_monitor_without_reading_an_event() {
        for kind in [u32::MAX, u32::MAX - 1] {
            let control = Control::default();
            control.require_monitor();
            control.monitor_started(false);
            let mut observer = Observer {
                input: InputState::default(),
                control: control.clone(),
                stop: CancellationToken::new(),
            };
            let returned = unsafe {
                callback(
                    std::ptr::null_mut(),
                    kind,
                    std::ptr::null_mut(),
                    (&mut observer as *mut Observer).cast(),
                )
            };
            assert!(returned.is_null());
            assert!(observer.stop.is_cancelled());
            assert_eq!(control.monitor_status()["reason"], "tap_disabled");
            assert_eq!(control.status()["mode"], "manual");
        }
    }
}

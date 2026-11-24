//! Process-owned Windows power requests for async host and daemon lifetimes.

use std::{
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use windows_sys::Win32::{
    Foundation::INVALID_HANDLE_VALUE,
    System::{
        Power::{
            ES_DISPLAY_REQUIRED, PowerClearRequest, PowerCreateRequest,
            PowerRequestDisplayRequired, PowerRequestSystemRequired, PowerSetRequest,
            SetThreadExecutionState,
        },
        SystemServices::POWER_REQUEST_CONTEXT_VERSION,
        Threading::{POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0},
    },
};

/// The OS owns the effective power policy. This guard owns our request.
pub struct PowerRequest {
    handle: OwnedHandle,
    system_required: bool,
    display_required: bool,
}

impl PowerRequest {
    pub fn keep_system_awake(reason: &str) -> io::Result<Self> {
        Self::new(reason, false)
    }

    pub fn keep_display_awake(reason: &str) -> io::Result<Self> {
        Self::new(reason, true)
    }

    fn new(reason: &str, display: bool) -> io::Result<Self> {
        let mut reason = reason.encode_utf16().chain([0]).collect::<Vec<_>>();
        let context = REASON_CONTEXT {
            Version: POWER_REQUEST_CONTEXT_VERSION,
            Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
            Reason: REASON_CONTEXT_0 {
                SimpleReasonString: reason.as_mut_ptr(),
            },
        };
        // SAFETY: the context and its NUL-terminated reason live through the call.
        let raw = unsafe { PowerCreateRequest(&context) };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: PowerCreateRequest returned a newly owned Kernel32 handle.
        let mut request = Self {
            handle: unsafe { OwnedHandle::from_raw_handle(raw) },
            system_required: false,
            display_required: false,
        };
        // SAFETY: the request owns the handle throughout both calls.
        if unsafe { PowerSetRequest(raw, PowerRequestSystemRequired) } == 0 {
            return Err(io::Error::last_os_error());
        }
        request.system_required = true;
        if display {
            if unsafe { PowerSetRequest(raw, PowerRequestDisplayRequired) } == 0 {
                return Err(io::Error::last_os_error());
            }
            request.display_required = true;
        }
        Ok(request)
    }
}

impl Drop for PowerRequest {
    fn drop(&mut self) {
        let raw = self.handle.as_raw_handle();
        // SAFETY: clear only successful requests before dropping the handle.
        unsafe {
            if self.display_required {
                PowerClearRequest(raw, PowerRequestDisplayRequired);
            }
            if self.system_required {
                PowerClearRequest(raw, PowerRequestSystemRequired);
            }
        }
    }
}

/// Reset the display idle timer once. The session retains its own power request.
pub fn wake_display() -> io::Result<()> {
    // SAFETY: the call takes only a documented flag. No persistent thread
    // assertion is installed, so an async task may resume on another thread.
    if unsafe { SetThreadExecutionState(ES_DISPLAY_REQUIRED) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

//! Controller-only, poll-based C ABI for the Swift iOS application.
//!
//! All calls for a handle are serialized by Swift's MainActor. No Rust task calls
//! Swift. Returned buffers have explicit ownership and survive session teardown.
//! The caller must free each result once and must not use a destroyed handle.

mod engine;
mod request;

use engine::Engine;
use serde_json::json;
use std::ffi::{CStr, CString, c_char};

fn string(value: impl ToString) -> *mut c_char {
    CString::new(value.to_string().replace('\0', "�"))
        .unwrap()
        .into_raw()
}

unsafe fn input<'a>(value: *const c_char) -> anyhow::Result<&'a str> {
    anyhow::ensure!(!value.is_null(), "Missing argument");
    // SAFETY: the C ABI requires a live NUL-terminated UTF-8 string for the call.
    Ok(unsafe { CStr::from_ptr(value) }.to_str()?)
}

/// Handle-independent storage call; safe to run on a background queue. Rust's
/// data-directory lock serializes it with mobile saves and other sync calls.
/// # Safety
/// Both inputs must be valid NUL-terminated strings for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rm_sync(path: *const c_char, command: *const c_char) -> *mut c_char {
    let result = std::panic::catch_unwind(|| -> anyhow::Result<_> {
        let path = unsafe { input(path)? };
        let command = unsafe { input(command)? };
        anyhow::ensure!(command.len() <= 8 * 1024 * 1024, "Command too large");
        Ok(removent_client::cloud_sync::dispatch(
            &removent_core::DataPaths { root: path.into() },
            serde_json::from_str(command)?,
        )?)
    });
    string(match result {
        Ok(Ok(value)) => json!({"ok":true,"value":value}),
        Ok(Err(error)) => json!({"ok":false,"error":error.to_string()}),
        Err(_) => json!({"ok":false,"error":"Sync storage failed"}),
    })
}

/// # Safety
/// `path`/`name` are NUL-terminated strings. `error` points to writable storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rm_create(
    path: *const c_char,
    name: *const c_char,
    error: *mut *mut c_char,
) -> *mut Engine {
    if error.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        *error = std::ptr::null_mut();
    }
    let result = std::panic::catch_unwind(|| -> anyhow::Result<Engine> {
        Engine::new(unsafe { input(path)? }, unsafe { input(name)? })
    });
    match result {
        Ok(Ok(engine)) => Box::into_raw(Box::new(engine)),
        Ok(Err(e)) => {
            unsafe {
                *error = string(format!("{e:#}"));
            }
            std::ptr::null_mut()
        }
        Err(_) => {
            unsafe {
                *error = string("Rust engine initialization failed");
            }
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// `engine` is a live, exclusively accessed handle; `command` is a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rm_call(engine: *mut Engine, command: *const c_char) -> *mut c_char {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> anyhow::Result<_> {
        let engine =
            unsafe { engine.as_mut() }.ok_or_else(|| anyhow::anyhow!("Engine unavailable"))?;
        let command = unsafe { input(command)? };
        anyhow::ensure!(command.len() <= 8 * 1024 * 1024, "Command too large");
        engine.command(serde_json::from_str(command)?)
    }));
    string(match result {
        Ok(Ok(value)) => json!({"ok":true, "value":value}),
        Ok(Err(e)) => json!({"ok":false, "error":format!("{e:#}")}),
        Err(_) => json!({"ok":false, "error":"Rust engine failed"}),
    })
}

/// # Safety
/// `engine` is a live, exclusively accessed handle. Free results with rm_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rm_poll_event(engine: *mut Engine) -> *mut c_char {
    let Some(engine) = (unsafe { engine.as_ref() }) else {
        return std::ptr::null_mut();
    };
    engine
        .shared
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .events
        .pop_front()
        .map(string)
        .unwrap_or(std::ptr::null_mut())
}

#[repr(C)]
pub struct Frame {
    pub data: *mut u8,
    pub len: usize,
    pub width: u32,
    pub height: u32,
    pub generation: u64,
}

/// # Safety
/// `engine` is a live handle. The owned BGRA result must be freed with rm_frame_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rm_take_frame(engine: *mut Engine) -> *mut Frame {
    let Some(engine) = (unsafe { engine.as_ref() }) else {
        return std::ptr::null_mut();
    };
    let mut state = engine
        .shared
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(frame) = state.frame.take() else {
        return std::ptr::null_mut();
    };
    if frame.width == 0
        || frame.height == 0
        || u64::from(frame.width) * u64::from(frame.height) * 4 != frame.data.len() as u64
    {
        return std::ptr::null_mut();
    }
    let len = frame.data.len();
    let data = Box::into_raw(frame.data.into_boxed_slice()) as *mut u8;
    Box::into_raw(Box::new(Frame {
        data,
        len,
        width: frame.width,
        height: frame.height,
        generation: state.generation,
    }))
}

#[repr(C)]
pub struct Audio {
    pub data: *mut i16,
    pub len: usize,
    pub sample_rate: u32,
    pub channels: u8,
    pub generation: u64,
}

/// # Safety
/// `engine` is live; free owned interleaved PCM with rm_audio_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rm_take_audio(engine: *mut Engine) -> *mut Audio {
    let Some(engine) = (unsafe { engine.as_ref() }) else {
        return std::ptr::null_mut();
    };
    let mut state = engine
        .shared
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let Some(pcm) = state.audio.pop_front() else {
        return std::ptr::null_mut();
    };
    let len = pcm.len();
    let data = Box::into_raw(pcm.into_boxed_slice()) as *mut i16;
    Box::into_raw(Box::new(Audio {
        data,
        len,
        sample_rate: state.audio_rate,
        channels: state.audio_channels,
        generation: state.generation,
    }))
}

/// # Safety
/// Pass only an unmodified rm_take_frame result, at most once. Null is allowed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rm_frame_free(frame: *mut Frame) {
    if !frame.is_null() {
        let frame = unsafe { Box::from_raw(frame) };
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(frame.data, frame.len)) });
    }
}

/// # Safety
/// Pass only an unmodified rm_take_audio result, at most once. Null is allowed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rm_audio_free(audio: *mut Audio) {
    if !audio.is_null() {
        let audio = unsafe { Box::from_raw(audio) };
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(audio.data, audio.len)) });
    }
}

/// # Safety
/// Pass only a string returned by this ABI, at most once. Null is allowed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rm_string_free(value: *mut c_char) {
    if !value.is_null() {
        drop(unsafe { CString::from_raw(value) });
    }
}

/// # Safety
/// Destroy a live handle once; no further calls may use it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rm_destroy(engine: *mut Engine) {
    if !engine.is_null() {
        drop(unsafe { Box::from_raw(engine) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abi_reports_bad_commands_and_buffers_outlive_the_engine() {
        unsafe {
            let dir = tempfile::tempdir().unwrap();
            let path = CString::new(dir.path().to_str().unwrap()).unwrap();
            let mut error = std::ptr::null_mut();
            let engine = rm_create(path.as_ptr(), c"mobile-test".as_ptr(), &mut error);
            assert!(!engine.is_null());
            assert!(error.is_null());
            let response = rm_call(engine, c"{invalid".as_ptr());
            let json: serde_json::Value =
                serde_json::from_str(CStr::from_ptr(response).to_str().unwrap()).unwrap();
            assert_eq!(json["ok"], false);
            rm_string_free(response);
            let shared = &(*engine).shared;
            shared.state.lock().unwrap().frame = Some(removent_client::DecodedFrame {
                data: vec![1, 2, 3, 255],
                width: 1,
                height: 1,
                pts_us: 0,
            });
            let frame = rm_take_frame(engine);
            rm_destroy(engine);
            assert_eq!(
                std::slice::from_raw_parts((*frame).data, (*frame).len),
                &[1, 2, 3, 255]
            );
            rm_frame_free(frame);
        }
    }
}

//! Core Foundation runtime / event-loop / stream wrappers.
//!
#![allow(clippy::missing_panics_doc, clippy::missing_errors_doc)]

//! ```rust,no_run
//! use apple_cf::cf::{
//!     CFFileDescriptor, CFMessagePort, CFNotificationCenter, CFReadStream, CFRunLoop,
//!     CFRunLoopRunResult, CFSocket, CFString, CFStreamPair, CFTimer, CFWriteStream,
//! };
//! use std::time::Duration;
//!
//! let center = CFNotificationCenter::local();
//! center.post(&CFString::new("com.doomfish.apple-cf.example"), None, false);
//!
//! let timer = CFTimer::new(Duration::from_millis(10), false);
//! let run_loop = CFRunLoop::current();
//! run_loop.add_timer(&timer);
//! let result = CFRunLoop::run_in_default_mode(Duration::from_millis(20), true);
//! assert!(matches!(result, CFRunLoopRunResult::HandledSource | CFRunLoopRunResult::TimedOut));
//!
//! let local = CFMessagePort::create_echo_local("com.doomfish.apple-cf.echo").unwrap();
//! let remote = CFMessagePort::connect_remote("com.doomfish.apple-cf.echo").unwrap();
//! let reply = remote.send_request(b"ping", Duration::from_millis(100)).unwrap();
//! assert_eq!(reply, b"ping");
//!
//! let pair = CFStreamPair::new(1024);
//! assert!(pair.read.open());
//! assert!(pair.write.open());
//! assert_eq!(pair.write.write(b"ok").unwrap(), 2);
//! let mut buffer = [0_u8; 2];
//! assert_eq!(pair.read.read(&mut buffer).unwrap(), 2);
//! assert_eq!(&buffer, b"ok");
//!
//! let socket = CFSocket::udp_ipv4().unwrap();
//! assert!(socket.is_valid());
//!
//! let stdin = std::io::stdin();
//! let fd = CFFileDescriptor::from_borrowed_fd(std::os::fd::AsFd::as_fd(&stdin)).unwrap();
//! assert_eq!(fd.native_descriptor(), 0);
//! ```

use super::base::impl_cf_type_wrapper;
use super::{CFDictionary, CFString};
use crate::ffi;
use crate::utils::panic_safe;
use std::collections::BTreeMap;
use std::ffi::{c_void, CString};
use std::os::fd::{AsRawFd, BorrowedFd, IntoRawFd, OwnedFd};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

impl_cf_type_wrapper!(CFNotificationCenter, cf_notification_center_get_type_id);
impl_cf_type_wrapper!(CFRunLoop, cf_run_loop_get_type_id);
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for CFRunLoop {}
unsafe impl Sync for CFRunLoop {}
impl_cf_type_wrapper!(CFTimer, cf_run_loop_timer_get_type_id);
impl_cf_type_wrapper!(CFMessagePort, cf_message_port_get_type_id);
impl_cf_type_wrapper!(CFReadStream, cf_read_stream_get_type_id);
impl_cf_type_wrapper!(CFWriteStream, cf_write_stream_get_type_id);
impl_cf_type_wrapper!(CFSocket, cf_socket_get_type_id);
impl_cf_type_wrapper!(CFFileDescriptor, cf_file_descriptor_get_type_id);

type NotificationCallback = dyn Fn(&CFString, Option<&CFDictionary>) + Send + Sync;

static NOTIFICATION_OBSERVERS: Mutex<BTreeMap<usize, Arc<NotificationCallback>>> =
    Mutex::new(BTreeMap::new());
static NEXT_NOTIFICATION_OBSERVER: AtomicUsize = AtomicUsize::new(1);
const CF_NOTIFICATION_SUSPENSION_BEHAVIOR_DELIVER_IMMEDIATELY: isize = 4;

extern "C" {
    fn CFNotificationCenterAddObserver(
        center: *mut c_void,
        observer: *const c_void,
        callback: extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *const c_void, *mut c_void),
        name: *mut c_void,
        object: *const c_void,
        suspension_behavior: isize,
    );
    fn CFNotificationCenterRemoveEveryObserver(center: *mut c_void, observer: *const c_void);
}

extern "C" fn notification_observer_trampoline(
    _center: *mut c_void,
    observer: *mut c_void,
    name: *mut c_void,
    _object: *const c_void,
    user_info: *mut c_void,
) {
    panic_safe::catch_user_panic("CFNotificationCenter observer", || {
        let callback = NOTIFICATION_OBSERVERS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(observer as usize))
            .cloned();
        let Some(callback) = callback else {
            return;
        };
        let Some(name) = (unsafe { CFString::from_raw_borrowed(name) }) else {
            return;
        };
        let user_info = unsafe { CFDictionary::from_raw_borrowed(user_info) };
        callback(&name, user_info.as_ref());
    });
}

pub struct CFNotificationObserver {
    center: CFNotificationCenter,
    token: usize,
}

impl Drop for CFNotificationObserver {
    fn drop(&mut self) {
        unsafe {
            CFNotificationCenterRemoveEveryObserver(
                self.center.as_ptr(),
                self.token as *const c_void,
            );
        }
        NOTIFICATION_OBSERVERS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.token);
    }
}

impl std::fmt::Debug for CFNotificationObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CFNotificationObserver")
            .field("center", &self.center.as_ptr())
            .field("token", &self.token)
            .finish()
    }
}

fn duration_to_seconds(duration: Duration) -> f64 {
    duration.as_secs_f64()
}

/// Result codes from `CFRunLoopRunInMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum CFRunLoopRunResult {
    Finished = 1,
    Stopped = 2,
    TimedOut = 3,
    HandledSource = 4,
}

impl CFNotificationCenter {
    /// Local notification center.
    #[must_use]
    pub fn local() -> Self {
        let ptr = unsafe { ffi::cf_notification_center_get_local() };
        unsafe { Self::from_raw(ptr) }.expect("CFNotificationCenterGetLocalCenter returned NULL")
    }

    /// Distributed notification center.
    #[cfg(target_os = "macos")]
    #[must_use]
    pub fn distributed() -> Self {
        let ptr = unsafe { ffi::cf_notification_center_get_distributed() };
        unsafe { Self::from_raw(ptr) }
            .expect("CFNotificationCenterGetDistributedCenter returned NULL")
    }

    /// Darwin notification center.
    #[must_use]
    pub fn darwin() -> Self {
        let ptr = unsafe { ffi::cf_notification_center_get_darwin() };
        unsafe { Self::from_raw(ptr) }
            .expect("CFNotificationCenterGetDarwinNotifyCenter returned NULL")
    }

    #[must_use]
    pub fn add_observer<F>(&self, name: &CFString, callback: F) -> CFNotificationObserver
    where
        F: Fn(&CFString, Option<&CFDictionary>) + Send + Sync + 'static,
    {
        let token = NEXT_NOTIFICATION_OBSERVER.fetch_add(1, Ordering::Relaxed);
        NOTIFICATION_OBSERVERS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(token, Arc::new(callback));
        unsafe {
            CFNotificationCenterAddObserver(
                self.as_ptr(),
                token as *const c_void,
                notification_observer_trampoline,
                name.as_ptr(),
                std::ptr::null(),
                CF_NOTIFICATION_SUSPENSION_BEHAVIOR_DELIVER_IMMEDIATELY,
            );
        }
        CFNotificationObserver {
            center: self.clone(),
            token,
        }
    }

    /// Post a notification with an optional user-info dictionary.
    pub fn post(
        &self,
        name: &CFString,
        user_info: Option<&CFDictionary>,
        deliver_immediately: bool,
    ) {
        unsafe {
            ffi::cf_notification_center_post_notification(
                self.as_ptr(),
                name.as_ptr(),
                user_info.map_or(std::ptr::null_mut(), CFDictionary::as_ptr),
                deliver_immediately,
            );
        }
    }
}

impl CFRunLoop {
    /// Current thread's run loop.
    #[must_use]
    pub fn current() -> Self {
        let ptr = unsafe { ffi::cf_run_loop_get_current() };
        unsafe { Self::from_raw(ptr) }.expect("CFRunLoopGetCurrent returned NULL")
    }

    /// Main thread run loop.
    #[must_use]
    pub fn main() -> Self {
        let ptr = unsafe { ffi::cf_run_loop_get_main() };
        unsafe { Self::from_raw(ptr) }.expect("CFRunLoopGetMain returned NULL")
    }

    /// Run the current thread's run loop in the default mode for `duration`.
    #[must_use]
    pub fn run_in_default_mode(
        duration: Duration,
        return_after_source_handled: bool,
    ) -> CFRunLoopRunResult {
        let code = unsafe {
            ffi::cf_run_loop_run_in_default_mode(
                duration_to_seconds(duration),
                return_after_source_handled,
            )
        };
        match code {
            1 => CFRunLoopRunResult::Finished,
            2 => CFRunLoopRunResult::Stopped,
            4 => CFRunLoopRunResult::HandledSource,
            _ => CFRunLoopRunResult::TimedOut,
        }
    }

    /// Wake the run loop.
    pub fn wake_up(&self) {
        unsafe { ffi::cf_run_loop_wake_up(self.as_ptr()) };
    }

    /// Stop the run loop.
    pub fn stop(&self) {
        unsafe { ffi::cf_run_loop_stop(self.as_ptr()) };
    }

    /// Add a timer to the run loop in the default mode.
    pub fn add_timer(&self, timer: &CFTimer) {
        unsafe { ffi::cf_run_loop_add_timer(self.as_ptr(), timer.as_ptr()) };
    }
}

impl CFTimer {
    /// Create a run-loop timer with a no-op callback.
    #[must_use]
    pub fn new(interval: Duration, repeats: bool) -> Self {
        let ptr = unsafe { ffi::cf_run_loop_timer_create(duration_to_seconds(interval), repeats) };
        unsafe { Self::from_raw(ptr) }.expect("CFRunLoopTimerCreate returned NULL")
    }

    /// Whether the timer is still valid.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        unsafe { ffi::cf_run_loop_timer_is_valid(self.as_ptr()) }
    }

    /// Fire the timer immediately.
    pub fn fire(&self) {
        unsafe { ffi::cf_run_loop_timer_fire(self.as_ptr()) };
    }

    /// Invalidate the timer.
    pub fn invalidate(&self) {
        unsafe { ffi::cf_run_loop_timer_invalidate(self.as_ptr()) };
    }
}

impl CFMessagePort {
    /// Create a local message port that echoes request data back as the reply.
    #[must_use]
    pub fn create_echo_local(name: &str) -> Option<Self> {
        let name = CString::new(name).ok()?;
        let ptr = unsafe { ffi::cf_message_port_create_echo_local(name.as_ptr()) };
        unsafe { Self::from_raw(ptr) }
    }

    /// Connect to an existing remote message port.
    #[must_use]
    pub fn connect_remote(name: &str) -> Option<Self> {
        let name = CString::new(name).ok()?;
        let ptr = unsafe { ffi::cf_message_port_create_remote(name.as_ptr()) };
        unsafe { Self::from_raw(ptr) }
    }

    /// Send a request and copy the reply bytes.
    pub fn send_request(&self, bytes: &[u8], timeout: Duration) -> Result<Vec<u8>, i32> {
        let mut out_bytes = std::ptr::null_mut();
        let mut out_len = 0_usize;
        let status = unsafe {
            ffi::cf_message_port_send_request(
                self.as_ptr(),
                bytes.as_ptr(),
                bytes.len(),
                duration_to_seconds(timeout),
                &raw mut out_bytes,
                &raw mut out_len,
            )
        };
        if status != 0 {
            return Err(status);
        }
        if out_bytes.is_null() {
            return Ok(Vec::new());
        }
        let reply = unsafe { std::slice::from_raw_parts(out_bytes, out_len) }.to_vec();
        unsafe { ffi::cf_message_port_free_bytes(out_bytes, out_len) };
        Ok(reply)
    }

    /// Invalidate the message port.
    pub fn invalidate(&self) {
        unsafe { ffi::cf_message_port_invalidate(self.as_ptr()) };
    }
}

/// Paired Core Foundation streams backed by a shared in-memory buffer.
#[derive(Debug, Clone)]
pub struct CFStreamPair {
    pub read: CFReadStream,
    pub write: CFWriteStream,
}

impl CFStreamPair {
    /// Create a bound read/write stream pair.
    #[must_use]
    pub fn new(transfer_buffer_size: usize) -> Self {
        let mut read = std::ptr::null_mut();
        let mut write = std::ptr::null_mut();
        unsafe { ffi::cf_stream_create_bound_pair(transfer_buffer_size, &raw mut read, &raw mut write) };
        Self {
            read: unsafe { CFReadStream::from_raw(read) }
                .expect("CFStreamCreateBoundPair read stream was NULL"),
            write: unsafe { CFWriteStream::from_raw(write) }
                .expect("CFStreamCreateBoundPair write stream was NULL"),
        }
    }
}

impl CFReadStream {
    /// Open the stream.
    #[must_use]
    pub fn open(&self) -> bool {
        unsafe { ffi::cf_read_stream_open(self.as_ptr()) }
    }

    /// Close the stream.
    pub fn close(&self) {
        unsafe { ffi::cf_read_stream_close(self.as_ptr()) };
    }

    /// Read bytes into `buffer`.
    pub fn read(&self, buffer: &mut [u8]) -> Result<usize, isize> {
        let count =
            unsafe { ffi::cf_read_stream_read(self.as_ptr(), buffer.as_mut_ptr(), buffer.len()) };
        if count < 0 {
            Err(count)
        } else {
            Ok(usize::try_from(count).expect("non-negative read count fits in usize"))
        }
    }
}

impl CFWriteStream {
    /// Open the stream.
    #[must_use]
    pub fn open(&self) -> bool {
        unsafe { ffi::cf_write_stream_open(self.as_ptr()) }
    }

    /// Close the stream.
    pub fn close(&self) {
        unsafe { ffi::cf_write_stream_close(self.as_ptr()) };
    }

    /// Write bytes from `buffer`.
    pub fn write(&self, buffer: &[u8]) -> Result<usize, isize> {
        let count =
            unsafe { ffi::cf_write_stream_write(self.as_ptr(), buffer.as_ptr(), buffer.len()) };
        if count < 0 {
            Err(count)
        } else {
            Ok(usize::try_from(count).expect("non-negative write count fits in usize"))
        }
    }
}

impl CFSocket {
    /// Create a UDP/IPv4 socket wrapper with no callbacks attached.
    #[must_use]
    pub fn udp_ipv4() -> Option<Self> {
        let ptr = unsafe { ffi::cf_socket_create_udp_ipv4() };
        unsafe { Self::from_raw(ptr) }
    }

    /// Native file descriptor.
    #[must_use]
    pub fn native(&self) -> i32 {
        unsafe { ffi::cf_socket_get_native(self.as_ptr()) }
    }

    /// Whether the socket is valid.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        unsafe { ffi::cf_socket_is_valid(self.as_ptr()) }
    }

    /// Invalidate the socket.
    pub fn invalidate(&self) {
        unsafe { ffi::cf_socket_invalidate(self.as_ptr()) };
    }
}

impl CFFileDescriptor {
    /// Wrap a native file descriptor with a no-op callback.
    #[must_use]
    pub fn from_owned_fd(fd: OwnedFd) -> Option<Self> {
        let ptr = unsafe { ffi::cf_file_descriptor_create(fd.as_raw_fd(), true) };
        let descriptor = unsafe { Self::from_raw(ptr) }?;
        let _ = fd.into_raw_fd();
        Some(descriptor)
    }

    #[must_use]
    pub fn from_borrowed_fd(fd: BorrowedFd<'_>) -> Option<Self> {
        let ptr = unsafe { ffi::cf_file_descriptor_create(fd.as_raw_fd(), false) };
        unsafe { Self::from_raw(ptr) }
    }

    /// Underlying native file descriptor.
    #[must_use]
    pub fn native_descriptor(&self) -> i32 {
        unsafe { ffi::cf_file_descriptor_get_native(self.as_ptr()) }
    }

    /// Invalidate the descriptor.
    pub fn invalidate(&self) {
        unsafe { ffi::cf_file_descriptor_invalidate(self.as_ptr()) };
    }
}

//! Opt-in WindowServer capture for login/lock-screen validation. CGDisplayStream
//! is deprecated since macOS 14, but remains available on supported systems.
//! It does not bypass Screen Recording permission or account authentication.
use block::{Block, ConcreteBlock};
use std::{
    ffi::{c_char, c_void},
    ptr,
    time::Instant,
};
use tokio::sync::mpsc;

type Object = *mut c_void;
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGDisplayStreamCreateWithDispatchQueue(
        display: u32,
        width: usize,
        height: usize,
        format: i32,
        properties: *const c_void,
        queue: Object,
        handler: *const c_void,
    ) -> Object;
    fn CGDisplayStreamStart(stream: Object) -> i32;
    fn CGDisplayStreamStop(stream: Object) -> i32;
}
#[link(name = "IOSurface", kind = "framework")]
unsafe extern "C" {
    fn IOSurfaceLock(surface: Object, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceUnlock(surface: Object, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceGetBaseAddress(surface: Object) -> Object;
    fn IOSurfaceGetBytesPerRow(surface: Object) -> usize;
    fn IOSurfaceGetWidth(surface: Object) -> usize;
    fn IOSurfaceGetHeight(surface: Object) -> usize;
    fn IOSurfaceGetAllocSize(surface: Object) -> usize;
}
unsafe extern "C" {
    fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> Object;
    fn dispatch_release(object: Object);
    fn CFRelease(object: *const c_void);
}

pub struct QuartzCapture {
    stream: Object,
    queue: Object,
    stopped: Option<mpsc::Receiver<String>>,
}
// CGDisplayStream uses its dedicated dispatch queue; ownership can move between
// Tokio workers. Stream APIs themselves are thread safe.
unsafe impl Send for QuartzCapture {}
impl QuartzCapture {
    pub fn take_stopped_rx(&mut self) -> Option<mpsc::Receiver<String>> {
        self.stopped.take()
    }
}
impl Drop for QuartzCapture {
    fn drop(&mut self) {
        // The stream retains its copied callback; callbacks own their channels,
        // never pointers to this struct. No unbounded wait for a stop callback.
        unsafe {
            CGDisplayStreamStop(self.stream);
            CFRelease(self.stream);
            dispatch_release(self.queue);
        }
    }
}

pub fn start(
    display: u32,
    width: u32,
    height: u32,
    frames: removent_core::latest::Sender<(Vec<u8>, i64)>,
) -> Result<QuartzCapture, String> {
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err("Invalid capture dimensions".into());
    }
    let (tx, rx) = mpsc::channel(1);
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let started = *EPOCH.get_or_init(Instant::now);
    let handler = ConcreteBlock::new(
        move |status: i32, _time: u64, surface: Object, _update: Object| {
            if status == 3 {
                let _ = tx.try_send("WindowServer capture stopped".into());
                return;
            }
            if status != 0 || surface.is_null() {
                return;
            }
            // SAFETY: IOSurface is valid during this callback. Read only while locked,
            // and validate dimensions, row stride and allocation size before copying.
            unsafe {
                if IOSurfaceLock(surface, 1, ptr::null_mut()) != 0 {
                    return;
                }
                let w = IOSurfaceGetWidth(surface);
                let h = IOSurfaceGetHeight(surface);
                let stride = IOSurfaceGetBytesPerRow(surface);
                let base = IOSurfaceGetBaseAddress(surface).cast::<u8>();
                let valid = w == width as usize
                    && h == height as usize
                    && !base.is_null()
                    && stride >= w * 4
                    && stride
                        .checked_mul(h)
                        .is_some_and(|n| n <= IOSurfaceGetAllocSize(surface));
                if valid {
                    let mut pixels = vec![0; w * h * 4];
                    for row in 0..h {
                        ptr::copy_nonoverlapping(
                            base.add(row * stride),
                            pixels.as_mut_ptr().add(row * w * 4),
                            w * 4,
                        );
                    }
                    let _ = frames.send((pixels, started.elapsed().as_micros() as i64));
                }
                IOSurfaceUnlock(surface, 1, ptr::null_mut());
            }
        },
    )
    .copy();
    unsafe {
        let queue = dispatch_queue_create(c"com.alkinum.removent.capture".as_ptr(), ptr::null());
        if queue.is_null() {
            return Err("Could not create capture queue".into());
        }
        let stream = CGDisplayStreamCreateWithDispatchQueue(
            display,
            width as usize,
            height as usize,
            i32::from_be_bytes(*b"BGRA"),
            ptr::null(),
            queue,
            &*handler as *const Block<(i32, u64, Object, Object), ()> as *const c_void,
        );
        if stream.is_null() {
            dispatch_release(queue);
            return Err(
                "WindowServer capture unavailable (check session and Screen Recording)".into(),
            );
        }
        let result = CGDisplayStreamStart(stream);
        if result != 0 {
            CFRelease(stream);
            dispatch_release(queue);
            return Err(format!("WindowServer capture start failed: {result}"));
        }
        Ok(QuartzCapture {
            stream,
            queue,
            stopped: Some(rx),
        })
    }
}

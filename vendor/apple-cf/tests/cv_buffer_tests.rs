use apple_cf::cf::CFString;
use apple_cf::cv::{
    CVAttachmentMode, CVBuffer, CVImageBuffer, CVMetalTextureCache, CVPixelBuffer,
    CVPixelBufferPool, CVPixelBufferPoolFlushFlags,
};
use std::collections::HashMap;
use std::sync::mpsc;
use std::thread;

#[test]
fn cv_buffer_attachment_round_trip() {
    let pixel_buffer = CVPixelBuffer::create(16, 16, 0x4247_5241).expect("pixel buffer");
    let buffer = CVBuffer::from_pixel_buffer(&pixel_buffer).expect("buffer");
    let key = CFString::new("com.doomfish.apple-cf.cv-buffer-tests");
    let value = CFString::new("attached");

    buffer.set_attachment(&key, &value, CVAttachmentMode::ShouldPropagate);
    assert!(buffer.attachment(&key).is_some());
    assert!(buffer
        .attachments(CVAttachmentMode::ShouldPropagate)
        .is_some());
    buffer.remove_all_attachments();
    assert!(buffer.attachment(&key).is_none());
}

#[test]
fn cv_image_buffer_smoke() {
    let pixel_buffer = CVPixelBuffer::create(8, 4, 0x4247_5241).expect("pixel buffer");
    let image_buffer = CVImageBuffer::from_pixel_buffer(&pixel_buffer).expect("image buffer");
    assert!((image_buffer.encoded_size().width - 8.0).abs() < f64::EPSILON);
    assert!((image_buffer.display_size().height - 4.0).abs() < f64::EPSILON);
}

#[test]
fn cv_metal_texture_cache_smoke() {
    let cache = CVMetalTextureCache::system_default().expect("system default Metal device");
    cache.flush();
}

fn allocation_threshold_key() -> String {
    let ptr = unsafe { apple_cf::raw::kCVPixelBufferPoolAllocationThresholdKey };
    unsafe { CFString::from_raw_borrowed(ptr.cast_mut().cast()) }
        .expect("allocation threshold key")
        .to_string()
}

#[test]
fn cv_pixel_buffer_pool_enforces_cap_and_reuses_buffers() {
    let pool = CVPixelBufferPool::create(16, 16, 0x4247_5241, 2).expect("pool");
    assert_eq!(pool.max_buffers(), 2);

    let first = pool.create_pixel_buffer().expect("first buffer");
    let second = pool.create_pixel_buffer().expect("second buffer");
    assert_eq!(pool.create_pixel_buffer().unwrap_err(), -6689);
    assert!(pool
        .try_create_pixel_buffer()
        .expect("threshold result")
        .is_none());

    drop(first);
    let reused = pool.create_pixel_buffer().expect("recycled buffer");
    drop((reused, second));
}

#[test]
fn cv_pixel_buffer_pool_cap_is_shared_by_clones_and_raw_imports() {
    let pool = CVPixelBufferPool::create(8, 8, 0x4247_5241, 1).expect("pool");
    let cloned = pool.clone();
    let first = cloned.create_pixel_buffer().expect("first buffer");
    assert_eq!(pool.create_pixel_buffer().unwrap_err(), -6689);

    let retained = unsafe { apple_cf::raw::CVPixelBufferPoolRetain(pool.as_ptr().cast()) };
    let imported = unsafe { CVPixelBufferPool::from_raw_with_max_buffers(retained.cast(), 1) }
        .expect("raw policy")
        .expect("retained pool");
    assert_eq!(imported.as_ptr(), pool.as_ptr());
    assert_eq!(imported.max_buffers(), 1);
    assert_eq!(imported.create_pixel_buffer().unwrap_err(), -6689);

    drop((first, cloned, pool));
    assert!(imported.create_pixel_buffer().is_ok());
}

#[test]
fn cv_pixel_buffer_pool_is_send_sync_with_cross_thread_cap() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CVPixelBufferPool>();

    let pool = CVPixelBufferPool::create(8, 8, 0x4247_5241, 2).expect("pool");
    let (ready_tx, ready_rx) = mpsc::channel();
    let mut releases = Vec::new();
    let mut workers = Vec::new();
    for _ in 0..2 {
        let pool = pool.clone();
        let ready_tx = ready_tx.clone();
        let (release_tx, release_rx) = mpsc::channel();
        releases.push(release_tx);
        workers.push(thread::spawn(move || {
            let buffer = pool.create_pixel_buffer().expect("worker allocation");
            ready_tx.send(pool.max_buffers()).expect("ready signal");
            release_rx.recv().expect("release signal");
            drop(buffer);
        }));
    }
    drop(ready_tx);

    assert_eq!(ready_rx.recv().expect("first worker"), 2);
    assert_eq!(ready_rx.recv().expect("second worker"), 2);
    assert_eq!(pool.create_pixel_buffer().unwrap_err(), -6689);
    for release in releases {
        release.send(()).expect("release worker");
    }

    for worker in workers {
        worker.join().expect("pool worker");
    }
    assert!(pool.create_pixel_buffer().is_ok());
}

#[test]
fn cv_pixel_buffer_pool_applies_per_call_thresholds_and_flush_flags() {
    let pool = CVPixelBufferPool::create(8, 8, 0x4247_5241, 2).expect("pool");
    let mut attributes = HashMap::new();
    attributes.insert(allocation_threshold_key(), 1);

    let first = pool
        .create_pixel_buffer_with_aux_attributes(Some(&attributes))
        .expect("first threshold allocation");
    assert_eq!(
        pool.create_pixel_buffer_with_aux_attributes(Some(&attributes))
            .unwrap_err(),
        -6689
    );

    pool.flush_with_flags(CVPixelBufferPoolFlushFlags::NONE);
    pool.flush_with_flags(CVPixelBufferPoolFlushFlags::EXCESS_BUFFERS);
    pool.flush_excess_buffers();
    assert!(pool.attributes().is_some());
    assert!(pool.pixel_buffer_attributes().is_some());

    drop(first);
    assert!(pool
        .create_pixel_buffer_with_aux_attributes(Some(&attributes))
        .is_ok());

    let capped = CVPixelBufferPool::create(8, 8, 0x4247_5241, 1).expect("capped pool");
    let held = capped.create_pixel_buffer().expect("held buffer");
    attributes.insert(allocation_threshold_key(), 3);
    assert_eq!(
        capped
            .create_pixel_buffer_with_aux_attributes(Some(&attributes))
            .unwrap_err(),
        -6689
    );
    drop(held);

    let invalid_attributes = HashMap::from([("invalid\0key".to_string(), 1)]);
    assert_eq!(
        pool.create_pixel_buffer_with_aux_attributes(Some(&invalid_attributes))
            .unwrap_err(),
        -50
    );
}

#[test]
fn cv_pixel_buffer_pool_zero_cap_is_unlimited() {
    let pool = CVPixelBufferPool::create(4, 4, 0x4247_5241, 0).expect("pool");
    assert_eq!(pool.max_buffers(), 0);
    let buffers = [
        pool.create_pixel_buffer().expect("unlimited allocation"),
        pool.create_pixel_buffer().expect("unlimited allocation"),
        pool.create_pixel_buffer().expect("unlimited allocation"),
        pool.create_pixel_buffer().expect("unlimited allocation"),
    ];
    assert_eq!(buffers.len(), 4);
}

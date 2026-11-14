//! Bounded, cancellable CPU capture for Windows and Linux X11 desktops.
use crate::{CapturedVideoFrame, health::CaptureHealth};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("screen capture: {0}")]
    Sc(String),
    #[error("display {0} not found")]
    DisplayNotFound(u32),
    #[error("screen capture permission: {0}")]
    NotAuthorized(String),
}
pub struct AudioFrame {
    pub samples: Vec<i16>,
    pub pts_micros: i64,
}
pub struct SckCapture {
    stop: Arc<AtomicBool>,
    stopped: Option<mpsc::Receiver<CaptureError>>,
    health: Arc<Mutex<CaptureHealth>>,
}
impl Drop for SckCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
impl SckCapture {
    pub fn take_stopped_rx(&mut self) -> Option<mpsc::Receiver<CaptureError>> {
        self.stopped.take()
    }
    pub fn health_failure(&self) -> Option<&'static str> {
        self.health.lock().unwrap().failure(Instant::now())
    }
}
pub fn request_screen_capture_access() -> Result<(), CaptureError> {
    xcap::Monitor::all()
        .map(|_| ())
        .map_err(|e| CaptureError::Sc(e.to_string()))
}
pub fn start_display_capture(
    id: u32,
    w: u32,
    h: u32,
    video: removent_core::latest::Sender<CapturedVideoFrame>,
    audio: Option<mpsc::Sender<AudioFrame>>,
) -> Result<SckCapture, CaptureError> {
    start_remote_capture(id, w, h, video, audio, 30)
}
pub fn start_remote_capture(
    id: u32,
    w: u32,
    h: u32,
    video: removent_core::latest::Sender<CapturedVideoFrame>,
    _audio: Option<mpsc::Sender<AudioFrame>>,
    fps: u8,
) -> Result<SckCapture, CaptureError> {
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return Err(CaptureError::NotAuthorized("Choose an X11 desktop session for remote hosting; Wayland portal sessions are not implemented".into()));
    }
    if w == 0 || h == 0 || fps == 0 {
        return Err(CaptureError::Sc(
            "invalid capture dimensions or frame rate".into(),
        ));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let health = Arc::new(Mutex::new(CaptureHealth::default()));
    let (tx, rx) = mpsc::channel(1);
    let cancel = stop.clone();
    let worker_health = health.clone();
    let generation = crate::next_capture_generation();
    std::thread::Builder::new()
        .name("desktop-capture".into())
        .spawn(move || {
            let monitor = match xcap::Monitor::all()
                .map_err(|e| CaptureError::Sc(e.to_string()))
                .and_then(|monitors| {
                    monitors
                        .into_iter()
                        .find(|m| m.id().ok() == Some(id))
                        .ok_or(CaptureError::DisplayNotFound(id))
                }) {
                Ok(monitor) => monitor,
                Err(error) => {
                    let _ = tx.blocking_send(error);
                    return;
                }
            };
            let start = Instant::now();
            let interval = Duration::from_secs_f64(1. / f64::from(fps.min(30)));
            while !cancel.load(Ordering::Relaxed) {
                let tick = Instant::now();
                let image = match monitor.capture_image() {
                    Ok(image) => image,
                    Err(e) => {
                        let _ = tx.blocking_send(CaptureError::Sc(e.to_string()));
                        break;
                    }
                };
                let image = if image.width() != w || image.height() != h {
                    image::imageops::resize(&image, w, h, image::imageops::FilterType::Triangle)
                } else {
                    image
                };
                let mut bgra = image.into_raw();
                for pixel in bgra.chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                }
                let mut frame = CapturedVideoFrame::new(
                    bgra,
                    w as usize,
                    h as usize,
                    start.elapsed().as_micros() as i64,
                );
                frame.generation = generation;
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                if video.send(frame).is_err() {
                    break;
                }
                worker_health.lock().unwrap().frame();
                std::thread::sleep(interval.saturating_sub(tick.elapsed()));
            }
        })
        .map_err(|e| CaptureError::Sc(e.to_string()))?;
    Ok(SckCapture {
        stop,
        stopped: Some(rx),
        health,
    })
}

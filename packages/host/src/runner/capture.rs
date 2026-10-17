//! Rebuild capture without replacing the authenticated control/media session.
use super::*;
use removent_media_capture::{
    AudioFrame, CaptureError, CapturedVideoFrame, SckCapture, quartz::QuartzCapture,
};
use removent_proto::DisplayInfo;
use tokio::sync::mpsc;

enum RunningCapture {
    Sck(SckCapture, mpsc::Receiver<CaptureError>),
    Quartz(QuartzCapture, mpsc::Receiver<String>),
}

impl RunningCapture {
    async fn stopped(&mut self) -> CaptureError {
        match self {
            Self::Sck(_, rx) => rx
                .recv()
                .await
                .unwrap_or_else(|| CaptureError::Sc("capture delegate closed".into())),
            Self::Quartz(_, rx) => CaptureError::Sc(
                rx.recv()
                    .await
                    .unwrap_or_else(|| "capture delegate closed".into()),
            ),
        }
    }
    fn health_failure(&self) -> Option<&'static str> {
        match self {
            Self::Sck(capture, _) => capture.health_failure(),
            Self::Quartz(capture, _) => capture.health_failure(),
        }
    }
}

#[derive(Default)]
struct RecoveryBudget {
    failures: u8,
}
impl RecoveryBudget {
    fn retry(&mut self, error: &CaptureError, healthy_for: Duration) -> bool {
        if matches!(error, CaptureError::NotAuthorized(_)) {
            return false;
        }
        if healthy_for >= Duration::from_secs(30) {
            self.failures = 0;
        }
        self.failures += 1;
        self.failures < 3
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run(
    mut display: DisplayInfo,
    legacy: bool,
    quality: removent_proto::VideoQuality,
    fps: u8,
    video: removent_core::latest::Sender<CapturedVideoFrame>,
    audio: Option<mpsc::Sender<AudioFrame>>,
    input: Option<Arc<dyn InputSink>>,
    cancel: CancellationToken,
) -> Result<(), CaptureError> {
    let mut budget = RecoveryBudget::default();
    let mut _wake_guard = super::display::DisplayAwake::new();
    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let (w, h) = quality.fit_dimensions(display.w_px, display.h_px);
        let id = display.id;
        let video = video.clone();
        let audio = audio.clone();
        let start = tokio::task::spawn_blocking(move || {
            if legacy {
                let mut stream = removent_media_capture::quartz::start(id as u32, w, h, video)
                    .map_err(CaptureError::Sc)?;
                let stopped = stream.take_stopped_rx().expect("new stream");
                Ok(RunningCapture::Quartz(stream, stopped))
            } else {
                let mut stream = removent_media_capture::sck::start_remote_capture(
                    id as u32, w, h, video, audio, fps,
                )?;
                let stopped = stream.take_stopped_rx().expect("new stream");
                Ok(RunningCapture::Sck(stream, stopped))
            }
        });
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            result = tokio::time::timeout(Duration::from_secs(8), start) => {
                result.map_err(|_| CaptureError::Sc("capture start timed out".into()))?
                    .map_err(|e| CaptureError::Sc(e.to_string()))?
            },
        };
        let started = std::time::Instant::now();
        let error = match result {
            Err(error) => error,
            Ok(mut capture) => {
                if let Some(input) = &input {
                    input.set_capture_display(id);
                }
                tracing::info!(
                    display_id = id,
                    width = w,
                    height = h,
                    "capture stream started"
                );
                let mut check = tokio::time::interval(Duration::from_secs(1));
                let failure = loop {
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => break None,
                        error = capture.stopped() => break Some(error),
                        _ = check.tick() => {
                            if let Some(reason) = capture.health_failure() {
                                break Some(CaptureError::Sc(reason.into()));
                            }
                            // Re-enumerate after topology/mode changes; an old ID
                            // or geometry must not keep a stale stream alive.
                            if !removent_input::display_list().iter().any(|d| {
                                d.id == id && d.w_px == display.w_px && d.h_px == display.h_px
                            }) {
                                break Some(CaptureError::Sc("capture display configuration changed".into()));
                            }
                        }
                    }
                };
                // stop_capture may block in the system framework; keep IPC/input responsive.
                let stop = tokio::task::spawn_blocking(move || drop(capture));
                tokio::time::timeout(Duration::from_secs(3), stop)
                    .await
                    .map_err(|_| CaptureError::Sc("capture stop timed out".into()))?
                    .map_err(|e| CaptureError::Sc(e.to_string()))?;
                let Some(failure) = failure else {
                    return Ok(());
                };
                failure
            }
        };
        if !budget.retry(&error, started.elapsed()) {
            return Err(error);
        }
        tracing::warn!(%error, attempt = budget.failures, "rebuilding capture stream");
        _wake_guard = super::display::DisplayAwake::new();
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            _ = tokio::time::sleep(Duration::from_secs(u64::from(budget.failures))) => {},
        }
        let refreshed = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            result = super::display::wait_for_display_id(Some(id)) => result,
        };
        match refreshed {
            Ok(next) => display = next,
            Err(error) => return Err(CaptureError::Sc(error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_is_bounded_and_does_not_restart_user_stopped_capture() {
        let mut budget = RecoveryBudget::default();
        let transient = CaptureError::Sc("interrupted".into());
        assert!(budget.retry(&transient, Duration::ZERO));
        assert!(budget.retry(&transient, Duration::ZERO));
        assert!(!budget.retry(&transient, Duration::ZERO));
        assert!(budget.retry(&transient, Duration::from_secs(30)));
        assert!(!budget.retry(
            &CaptureError::NotAuthorized("stopped".into()),
            Duration::from_secs(30)
        ));
    }
}

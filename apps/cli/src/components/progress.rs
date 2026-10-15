use super::{Tone, dimensions, enabled, fit, paint};
use std::future::Future;
use std::io::{self, IsTerminal, Write};
use std::time::Duration;

struct Line(bool);
impl Drop for Line {
    fn drop(&mut self) {
        if self.0 {
            let _ = write!(io::stderr(), "\r\x1b[2K");
            let _ = io::stderr().flush();
        }
    }
}

/// A delayed activity indicator. Fast commands stay quiet; slow requests animate
/// one owned line, removed before either a result or an error is presented.
pub(crate) async fn run<T>(message: &str, human: bool, future: impl Future<Output = T>) -> T {
    if !human || !enabled() || !io::stderr().is_terminal() {
        return future.await;
    }
    tokio::pin!(future);
    let delay = tokio::time::sleep(Duration::from_millis(180));
    tokio::pin!(delay);
    tokio::select! { result = &mut future => return result, _ = &mut delay => {} }
    let _line = Line(true);
    let mut frame = 0;
    let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    loop {
        let text = fit(message, dimensions().0.saturating_sub(4));
        let _ = write!(
            io::stderr(),
            "\r\x1b[2K  {} {text}",
            paint(frames[frame % frames.len()], Tone::Accent)
        );
        let _ = io::stderr().flush();
        frame += 1;
        tokio::select! {
            result = &mut future => return result,
            _ = tokio::time::sleep(Duration::from_millis(80)) => {}
        }
    }
}

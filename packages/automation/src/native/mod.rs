use crate::{Outcome, Provider, check_cancel};
use removent_proto::automation::{Command, Failure};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
mod accessibility;
mod input;
pub(super) mod monitor;
mod screen;

pub struct Native {
    runtime: tokio::runtime::Handle,
    screen: screen::Screen,
    ax: accessibility::Accessibility,
    control: crate::control::Control,
}
impl Native {
    pub fn new(runtime: tokio::runtime::Handle, control: crate::control::Control) -> Self {
        Self {
            runtime,
            screen: Default::default(),
            ax: Default::default(),
            control,
        }
    }
    fn run(&mut self, command: &Command, cancel: &CancellationToken) -> Outcome {
        check_cancel(cancel)?;
        if matches!(command, Command::Capabilities {}) {
            let trusted = removent_input::accessibility_trusted();
            return Ok(
                json!({"version":1,"screen":screen::permitted(),"input":trusted && self.control.monitor_ready(),"native_ui":trusted,
                "can_mutate":trusted && self.control.can_mutate(),"local_input_monitor":self.control.monitor_status(),
                "browser":{"ready":false,"reason":"extension_not_connected"},
                "control_handover":true,"coordinate_space":"screenshot_pixels","operation_ack":"native_api_returned"}),
            );
        }
        if matches!(command, Command::State {}) {
            return Ok(
                json!({"apps":accessibility::apps()?,"displays":removent_input::display_list(),
                "accessibility":removent_input::accessibility_trusted(),"screen_recording":screen::permitted(),
                "local_input_monitor":self.control.monitor_status()}),
            );
        }
        if let Command::Screenshot {
            display_id,
            max_width,
        } = command
        {
            return self
                .screen
                .capture(*display_id, *max_width, &self.runtime, cancel);
        }
        if !removent_input::accessibility_trusted() {
            return Err(Failure::new(
                "permission_denied",
                "Enable Accessibility for the Removent host",
            ));
        }
        match command {
            Command::OpenApp { bundle_id } => {
                let mut child = std::process::Command::new("/usr/bin/open")
                    .args(["-b", bundle_id])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .map_err(|_| Failure::new("launch_failed", "Cannot launch application"))?;
                let start = Instant::now();
                loop {
                    if cancel.is_cancelled() || start.elapsed() > Duration::from_secs(5) {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(Failure::new(
                            "outcome_unknown",
                            "Application launch interrupted; refresh state",
                        ));
                    }
                    match child.try_wait() {
                        Ok(Some(status)) if status.success() => {
                            return Ok(json!({"status":"executed","bundle_id":bundle_id}));
                        }
                        Ok(Some(_)) | Err(_) => {
                            return Err(Failure::new(
                                "launch_failed",
                                "Application was not found or could not be opened",
                            ));
                        }
                        Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                    }
                }
            }
            Command::Snapshot { target } => self.ax.snapshot(target, cancel),
            Command::ElementAction { reference, name } => self.ax.action(reference, name),
            Command::Focus { reference } => self.ax.focus(reference),
            Command::SetValue { reference, value } => self.ax.set_value(reference, value),
            Command::SelectText {
                reference,
                text,
                prefix,
                suffix,
                placement,
            } => self.ax.select(
                reference,
                text,
                prefix.as_deref(),
                suffix.as_deref(),
                *placement,
            ),
            _ => input::execute(command, &self.screen, cancel, &self.control),
        }
    }
}
impl Provider for Native {
    fn invalidate_observations(&mut self) {
        self.screen = Default::default();
        self.ax = Default::default();
    }
    fn execute(&mut self, command: &Command, cancel: &CancellationToken) -> Outcome {
        objc2::rc::autoreleasepool(|_| self.run(command, cancel))
    }
}
fn executed() -> Outcome {
    Ok(
        json!({"status":"executed","acknowledgement":"native_api_returned","verify_with":"snapshot_or_screenshot"}),
    )
}
fn failure(message: impl Into<String>) -> Failure {
    Failure::new("native_error", message)
}

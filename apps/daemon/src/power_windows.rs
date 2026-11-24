use removent_core::windows_power::PowerRequest;

#[derive(Default)]
pub struct IdleSleepAssertion {
    request: Option<PowerRequest>,
    error: Option<String>,
}

impl IdleSleepAssertion {
    pub fn set_enabled(&mut self, enabled: bool) {
        if !enabled {
            self.request = None;
            self.error = None;
        } else if self.request.is_none() {
            match PowerRequest::keep_system_awake("Removent background remote access") {
                Ok(request) => {
                    self.request = Some(request);
                    self.error = None;
                }
                Err(error) => {
                    tracing::warn!(%error, "could not apply background power policy");
                    self.error = Some(error.to_string());
                }
            }
        }
    }

    pub fn is_active(&self) -> bool {
        self.request.is_some()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

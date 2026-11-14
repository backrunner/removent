//! Power inhibition is not silently reported as active on unsupported backends.
#[derive(Default)]
pub struct IdleSleepAssertion {
    enabled: bool,
}
impl IdleSleepAssertion {
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
    pub fn is_active(&self) -> bool {
        false
    }
    pub fn error(&self) -> Option<&str> {
        self.enabled
            .then_some("Idle sleep prevention is not available on this platform")
    }
}

//! Process-owned idle-sleep policy. Never changes the user's Energy Saver settings.
use core_foundation::{
    base::TCFType,
    string::{CFString, CFStringRef},
};

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPMAssertionCreateWithName(
        kind: CFStringRef,
        level: u32,
        name: CFStringRef,
        id: *mut u32,
    ) -> i32;
    fn IOPMAssertionRelease(id: u32) -> i32;
}

#[derive(Default)]
pub struct IdleSleepAssertion {
    id: Option<u32>,
    error: Option<String>,
}

impl IdleSleepAssertion {
    pub fn set_enabled(&mut self, enabled: bool) {
        if !enabled {
            if let Some(id) = self.id.take() {
                // SAFETY: this object owns the successful assertion until release.
                unsafe {
                    IOPMAssertionRelease(id);
                }
            }
            self.error = None;
        } else if self.id.is_none() {
            let kind = CFString::new("PreventUserIdleSystemSleep");
            let name = CFString::new("Removent background remote access");
            let mut id = 0;
            // SAFETY: CFStrings and the output pointer outlive the synchronous call.
            let result = unsafe {
                IOPMAssertionCreateWithName(
                    kind.as_concrete_TypeRef(),
                    255,
                    name.as_concrete_TypeRef(),
                    &mut id,
                )
            };
            if result == 0 {
                self.id = Some(id);
                self.error = None;
                tracing::info!("preventing idle system sleep while removentd runs");
            } else {
                self.error = Some(format!(
                    "Could not prevent idle system sleep (IOKit {result})"
                ));
                tracing::warn!(result, "could not apply background power policy");
            }
        }
    }

    pub fn is_active(&self) -> bool {
        self.id.is_some()
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

impl Drop for IdleSleepAssertion {
    fn drop(&mut self) {
        self.set_enabled(false);
    }
}

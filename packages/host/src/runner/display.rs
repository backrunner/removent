//! Wake and retain the capture display for an admitted remote session.

use crate::session::HostError;
use core_foundation::{
    base::TCFType,
    string::{CFString, CFStringRef},
};
use removent_proto::DisplayInfo;
use std::time::Duration;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPMAssertionDeclareUserActivity(name: CFStringRef, user_type: u32, id: *mut u32) -> i32;
    fn IOPMAssertionCreateWithName(
        kind: CFStringRef,
        level: u32,
        name: CFStringRef,
        id: *mut u32,
    ) -> i32;
    fn IOPMAssertionRelease(id: u32) -> i32;
}

/// Power assertions are scoped to the session, including failed negotiation,
/// peer disconnect, and cancellation. This never changes Energy Saver settings.
pub(super) struct DisplayAwake(Vec<u32>);

impl DisplayAwake {
    pub(super) fn new() -> Self {
        let name = CFString::new("Removent remote desktop capture");
        let kind = CFString::new("PreventUserIdleDisplaySleep");
        let mut assertions = Self(Vec::new());
        let mut id = 0;
        // Remote user activity wakes an already sleeping display. The second
        // assertion alone only prevents future idle sleep; it cannot wake it.
        let result =
            unsafe { IOPMAssertionDeclareUserActivity(name.as_concrete_TypeRef(), 1, &mut id) };
        if result == 0 {
            assertions.0.push(id);
        } else {
            tracing::warn!(result, "could not wake capture display");
        }
        let mut id = 0;
        let result = unsafe {
            IOPMAssertionCreateWithName(
                kind.as_concrete_TypeRef(),
                255,
                name.as_concrete_TypeRef(),
                &mut id,
            )
        };
        if result == 0 {
            assertions.0.push(id);
        } else {
            tracing::warn!(result, "could not retain awake capture display");
        }
        assertions
    }
}

impl Drop for DisplayAwake {
    fn drop(&mut self) {
        for id in &self.0 {
            unsafe {
                IOPMAssertionRelease(*id);
            }
        }
    }
}

pub(super) async fn wait_for_display() -> Result<DisplayInfo, HostError> {
    wait_for_display_with(removent_input::display_list, Duration::from_secs(5)).await
}

async fn wait_for_display_with(
    mut displays: impl FnMut() -> Vec<DisplayInfo>,
    timeout: Duration,
) -> Result<DisplayInfo, HostError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let list = displays();
        if let Some(selected) = list
            .iter()
            .filter(|d| d.id != 0 && d.w_px > 0 && d.h_px > 0)
            .max_by_key(|d| d.is_main)
        {
            tracing::info!(
                display_id = selected.id,
                width = selected.w_px,
                height = selected.h_px,
                "capture display ready"
            );
            return Ok(selected.clone());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(HostError::Timeout("active capture display after wake"));
        }
        tokio::time::sleep_until(
            (tokio::time::Instant::now() + Duration::from_millis(100)).min(deadline),
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(id: u64, is_main: bool) -> DisplayInfo {
        DisplayInfo {
            id,
            w_px: 1920,
            h_px: 1080,
            scale: 1.,
            dpi: 96,
            is_main,
        }
    }

    #[tokio::test]
    async fn sleeping_display_waits_for_real_main_display() {
        let mut polls = 0;
        let resolved = wait_for_display_with(
            || {
                polls += 1;
                if polls < 3 {
                    vec![]
                } else {
                    vec![display(8, false), display(2, true)]
                }
            },
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert_eq!(resolved.id, 2);
        assert_eq!(polls, 3);
    }

    #[tokio::test]
    async fn absent_display_times_out_without_fabricating_id_one() {
        let result = wait_for_display_with(Vec::new, Duration::from_millis(20)).await;
        assert!(matches!(result, Err(HostError::Timeout(_))));
    }
}

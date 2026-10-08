//! macOS TCC permission helpers for the headless daemon (Screen Recording /
//! Accessibility), exposed over IPC so management clients can surface missing
//! permissions instead of failing silently.

use crate::state::DaemonState;
use removent_core::ipc::{HostPermission, IpcResponse};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

static REQUEST_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Persist each first request before opening a dialog, so a denied request or
/// a launchd restart never creates a prompt loop. Explicit requests bypass it.
fn claim_initial_request(root: &Path, name: &str) -> std::io::Result<bool> {
    // Separate the stable helper identity from older, unbundled removentd
    // requests. A request for that old identity cannot authorize this host.
    let directory = root.join("permissions/host-v1");
    std::fs::create_dir_all(&directory)?;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join(format!("{name}-requested")))
    {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn request_initial_permissions(state: &DaemonState) {
    if !state.login_window && state.enabled.load(Ordering::SeqCst) {
        let _ = start_request(state.paths.root.clone(), None, true);
    }
}

pub fn request_interactive_permissions(
    state: &DaemonState,
    permission: Option<HostPermission>,
) -> IpcResponse {
    if state.login_window {
        return IpcResponse::Error {
            message: rust_i18n::t!("error.permission_login_required").to_string(),
        };
    }
    match start_request(state.paths.root.clone(), permission, false) {
        Ok(()) => IpcResponse::Ok,
        Err(message) => IpcResponse::Error { message },
    }
}

fn start_request(
    root: std::path::PathBuf,
    permission: Option<HostPermission>,
    initial: bool,
) -> Result<(), String> {
    if REQUEST_ACTIVE.swap(true, Ordering::SeqCst) {
        return Err(rust_i18n::t!("error.permission_request_active").to_string());
    }
    // Do not use spawn_blocking: runtime shutdown must not wait for a user to
    // answer an OS dialog. The process owns this detached thread's lifetime.
    let result = std::thread::Builder::new()
        .name("host-permissions".into())
        .spawn(move || {
            struct Reset;
            impl Drop for Reset {
                fn drop(&mut self) {
                    REQUEST_ACTIVE.store(false, Ordering::SeqCst);
                }
            }
            let _reset = Reset;
            let permissions = permission.map_or_else(
                || {
                    vec![
                        HostPermission::Accessibility,
                        HostPermission::ScreenRecording,
                    ]
                },
                |permission| vec![permission],
            );
            for permission in permissions {
                let (name, granted, request, pane): (&str, bool, fn() -> bool, &str) =
                    match permission {
                        HostPermission::ScreenRecording => (
                            "screen-recording",
                            screen_recording_granted(),
                            request_screen_recording,
                            "Privacy_ScreenCapture",
                        ),
                        HostPermission::Accessibility => (
                            "accessibility",
                            accessibility_granted(),
                            request_accessibility,
                            "Privacy_Accessibility",
                        ),
                    };
                if granted {
                    continue;
                }
                match claim_initial_request(&root, name) {
                    Ok(false) if initial => continue,
                    Err(error) => {
                        tracing::warn!(%error, name, "could not persist permission request");
                        if initial {
                            continue;
                        }
                    }
                    _ => {}
                }
                tracing::info!(name, initial, "requesting host permission from removentd");
                let granted = request();
                // macOS may suppress a repeated consent dialog after denial.
                // A manual action still takes the user to its actionable pane.
                if !initial && !granted {
                    let _ = std::process::Command::new("/usr/bin/open")
                        .arg(format!(
                            "x-apple.systempreferences:com.apple.preference.security?{pane}"
                        ))
                        .status();
                }
            }
        });
    if let Err(error) = result {
        REQUEST_ACTIVE.store(false, Ordering::SeqCst);
        return Err(error.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_host_and_login_window_never_start_consent() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = DaemonState::new(
            removent_core::DataPaths {
                root: directory.path().into(),
            },
            removent_core::Settings {
                host_enabled: false,
                ..Default::default()
            },
            "test".into(),
        );
        request_initial_permissions(&state);
        assert!(!directory.path().join("permissions").exists());
        state.login_window = true;
        state.enabled.store(true, Ordering::SeqCst);
        request_initial_permissions(&state);
        assert!(matches!(
            request_interactive_permissions(&state, None),
            IpcResponse::Error { .. }
        ));
        assert!(!directory.path().join("permissions").exists());
    }

    #[test]
    fn initial_requests_are_recorded_per_permission_and_survive_restart() {
        let directory = tempfile::tempdir().unwrap();
        assert!(claim_initial_request(directory.path(), "screen-recording").unwrap());
        assert!(!claim_initial_request(directory.path(), "screen-recording").unwrap());
        assert!(claim_initial_request(directory.path(), "accessibility").unwrap());
        assert!(!claim_initial_request(directory.path(), "accessibility").unwrap());
    }

    #[test]
    fn unwritable_request_state_is_not_treated_as_a_first_launch() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("permissions"), "not a directory").unwrap();
        assert!(claim_initial_request(directory.path(), "screen-recording").is_err());
    }
}

/// Screen Recording permission (preflight only, never prompts).
#[cfg(target_os = "macos")]
pub fn screen_recording_granted() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// Accessibility permission (preflight only, never prompts).
#[cfg(target_os = "macos")]
pub fn accessibility_granted() -> bool {
    accessibility_trusted(false)
}

/// Trigger the system consent prompt for Screen Recording.
#[cfg(target_os = "macos")]
pub fn request_screen_recording() -> bool {
    if unsafe { CGRequestScreenCaptureAccess() } {
        return true;
    }
    match removent_media_capture::sck::request_screen_capture_access() {
        Ok(()) => screen_recording_granted(),
        Err(error) => {
            tracing::info!(%error, "ScreenCaptureKit permission request was not granted");
            false
        }
    }
}

/// Trigger the system consent prompt for Accessibility
/// (AXIsProcessTrustedWithOptions with kAXTrustedCheckOptionPrompt = true).
#[cfg(target_os = "macos")]
pub fn request_accessibility() -> bool {
    accessibility_trusted(true)
}

#[cfg(target_os = "macos")]
fn accessibility_trusted(prompt: bool) -> bool {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;

    // SAFETY: kAXTrustedCheckOptionPrompt is a valid, immutable extern
    // CFStringRef; wrap_under_get_rule borrows it without retaining.
    let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
    let value = if prompt {
        CFBoolean::true_value()
    } else {
        CFBoolean::false_value()
    };
    let options = CFDictionary::from_CFType_pairs(&[(key, value)]);
    // SAFETY: options is a valid CFDictionaryRef for the duration of the call.
    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) }
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: core_foundation::dictionary::CFDictionaryRef)
    -> bool;
    static kAXTrustedCheckOptionPrompt: core_foundation::string::CFStringRef;
}

// The daemon is macOS-only in practice; stubs keep the lib compiling elsewhere.
#[cfg(not(target_os = "macos"))]
pub fn screen_recording_granted() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
pub fn accessibility_granted() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
pub fn request_screen_recording() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
pub fn request_accessibility() -> bool {
    true
}

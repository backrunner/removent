//! macOS TCC permission helpers for the headless daemon (Screen Recording /
//! Accessibility), exposed over IPC so management clients can surface missing
//! permissions instead of failing silently.

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
    unsafe { CGRequestScreenCaptureAccess() }
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

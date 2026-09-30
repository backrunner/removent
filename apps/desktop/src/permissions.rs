//! macOS TCC permission checks (FR-55): Screen Recording / Accessibility.

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: core_foundation::dictionary::CFDictionaryRef)
    -> bool;
    static kAXTrustedCheckOptionPrompt: core_foundation::string::CFStringRef;
}

/// Screen Recording permission (required for controlled-side capture).
pub fn screen_capture_granted() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// Trigger the system consent prompt (call only when not yet granted).
pub fn request_screen_capture() -> bool {
    unsafe { CGRequestScreenCaptureAccess() }
}

/// Accessibility permission (required for controlled-side input injection).
pub fn accessibility_granted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Trigger the system consent prompt for Accessibility
/// (AXIsProcessTrustedWithOptions with kAXTrustedCheckOptionPrompt = true).
pub fn request_accessibility() -> bool {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;

    // SAFETY: kAXTrustedCheckOptionPrompt is a valid, immutable extern
    // CFStringRef; wrap_under_get_rule borrows it without retaining.
    let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
    let options = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::true_value())]);
    // SAFETY: options is a valid CFDictionaryRef for the duration of the call.
    unsafe { AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) }
}

#[derive(Clone, Copy)]
pub enum PermissionKind {
    ScreenCapture,
    Accessibility,
}

impl PermissionKind {
    pub fn granted(&self) -> bool {
        match self {
            Self::ScreenCapture => screen_capture_granted(),
            Self::Accessibility => accessibility_granted(),
        }
    }

    /// Stable element-id slug (not localized).
    pub fn slug(&self) -> &'static str {
        match self {
            Self::ScreenCapture => "screen-capture",
            Self::Accessibility => "accessibility",
        }
    }

    pub fn title(&self) -> String {
        match self {
            Self::ScreenCapture => rust_i18n::t!("permissions.screen_capture").to_string(),
            Self::Accessibility => rust_i18n::t!("permissions.accessibility").to_string(),
        }
    }

    pub fn purpose(&self) -> String {
        match self {
            Self::ScreenCapture => rust_i18n::t!("permissions.screen_capture_purpose").to_string(),
            Self::Accessibility => rust_i18n::t!("permissions.accessibility_purpose").to_string(),
        }
    }

    /// Trigger the system consent flow (Screen Recording via
    /// CGRequestScreenCaptureAccess, Accessibility via
    /// AXIsProcessTrustedWithOptions prompt), then open the corresponding
    /// System Settings pane.
    pub fn request_and_open_settings(&self) {
        if !self.granted() {
            match self {
                Self::ScreenCapture => {
                    let _ = request_screen_capture();
                }
                Self::Accessibility => {
                    let _ = request_accessibility();
                }
            }
        }
        self.open_settings();
    }

    /// Jump to the corresponding System Settings pane.
    pub fn open_settings(&self) {
        let anchor = match self {
            Self::ScreenCapture => "Privacy_ScreenCapture",
            Self::Accessibility => "Privacy_Accessibility",
        };
        let _ = std::process::Command::new("open")
            .arg(format!(
                "x-apple.systempreferences:com.apple.preference.security?{anchor}"
            ))
            .spawn();
    }
}

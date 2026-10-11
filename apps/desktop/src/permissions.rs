//! Hosting permissions are checked and requested by removentd over IPC.

#[derive(Clone, Copy)]
pub enum PermissionKind {
    ScreenCapture,
    Accessibility,
}

pub fn setup_hint(grants: Option<(bool, bool)>) -> String {
    match grants {
        Some((true, false)) => rust_i18n::t!("permissions.accessibility_setup_hint").to_string(),
        Some((false, true)) => rust_i18n::t!("permissions.screen_capture_setup_hint").to_string(),
        _ => rust_i18n::t!("permissions.daemon_setup_hint").to_string(),
    }
}

impl PermissionKind {
    pub fn host_permission(self) -> removent_core::ipc::HostPermission {
        match self {
            Self::ScreenCapture => removent_core::ipc::HostPermission::ScreenRecording,
            Self::Accessibility => removent_core::ipc::HostPermission::Accessibility,
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
}

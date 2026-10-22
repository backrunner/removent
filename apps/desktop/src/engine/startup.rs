use super::*;
use removent_core::service::Service;
use removent_core::system_service::{self, SystemServiceStatus};
use std::path::Path;

mod system;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StartupSettings {
    pub launch_at_login: Option<bool>,
    pub busy: bool,
    pub error: Option<String>,
    pub system: Option<SystemServiceStatus>,
    pub approval_outdated: bool,
}

impl Engine {
    #[cfg(test)]
    pub(crate) fn set_startup_settings_for_test(&self, state: StartupSettings) {
        *self.startup.lock().unwrap() = state;
    }

    pub fn startup_settings(&self) -> StartupSettings {
        self.startup.lock().unwrap().clone()
    }

    pub fn startup_is_managed_externally(&self) -> bool {
        std::env::var("REMOVENT_DEV_SUPERVISED").as_deref() == Ok("1")
    }

    pub fn refresh_startup_settings(&self) {
        self.update_startup_settings(|_| Ok(()));
    }

    pub fn set_launch_at_login(&self, on: bool) {
        self.update_startup_settings(move |service| service.set_launch_at_login(on));
    }

    pub(super) fn start_managed_daemon(&self) {
        let engine = self.clone();
        self.update_startup_settings(move |service| {
            // Also cover direct execution of the installed desktop binary, which
            // bypasses the Swift launcher. Development builds stay transient.
            let defaults = match (std::env::current_exe(), std::env::var_os("HOME")) {
                (Ok(exe), Some(home)) if is_installed_app(&exe, Path::new(&home)) => {
                    service.install_defaults()
                }
                _ => Ok(()),
            };
            // A login-registration failure must not prevent the current session
            // from hosting. Keep the error visible beside the startup setting.
            let running = service.ensure_running();
            defaults.and(running)?;
            engine.install_system_startup_defaults()
        });
    }

    fn update_startup_settings(
        &self,
        operation: impl FnOnce(&Service) -> Result<()> + Send + 'static,
    ) {
        if self.startup_is_managed_externally() {
            return;
        }
        {
            let mut state = self.startup.lock().unwrap();
            // Serialize refresh and mutation so a stale read cannot undo the
            // result of a click. All windows share this state.
            if state.busy {
                return;
            }
            state.busy = true;
            state.error = None;
        }
        let _ = self.events_tx.send(UiEvent::StartupChanged);
        let engine = self.clone();
        self.rt.spawn_blocking(move || {
            let service = Self::daemon_binary()
                .ok_or_else(|| anyhow::anyhow!(t!("notice.daemon_binary_missing").to_string()))
                .and_then(|bin| Service::new(engine.paths.clone(), bin));
            let (enabled, result) = match service {
                Ok(service) => {
                    let result = operation(&service);
                    let status = service.status().map(|status| status.launch_at_login);
                    (
                        status.as_ref().ok().copied(),
                        result.and(status.map(|_| ())),
                    )
                }
                Err(error) => (None, Err(error)),
            };
            let error = result.err().map(|error| {
                tracing::warn!(%error, "background service startup setting failed");
                t!("settings.startup_failed", error = format!("{error:#}")).to_string()
            });
            let system = system_service::status();
            let error = error.or_else(|| {
                system.as_ref().err().map(|error| {
                    t!("settings.startup_failed", error = format!("{error:#}")).to_string()
                })
            });
            let system = system.ok();
            let approval_outdated = system.as_ref().is_some_and(|status| {
                status.installed
                    && system_service::current_approved_devices(&engine.paths)
                        .is_ok_and(|current| current != status.approved_devices)
            });
            *engine.startup.lock().unwrap() = StartupSettings {
                launch_at_login: enabled,
                busy: false,
                error,
                system,
                approval_outdated,
            };
            let _ = engine.events_tx.send(UiEvent::StartupChanged);
        });
    }
}

fn is_installed_app(executable: &Path, home: &Path) -> bool {
    let Some(macos) = executable
        .parent()
        .filter(|p| p.ends_with("Contents/MacOS"))
    else {
        return false;
    };
    let Some(bundle) = macos.parent().and_then(Path::parent) else {
        return false;
    };
    bundle.extension().is_some_and(|ext| ext == "app")
        && bundle.parent().is_some_and(|directory| {
            directory == Path::new("/Applications") || directory == home.join("Applications")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_cover_installed_binaries_without_registering_builds_or_disk_images() {
        let home = Path::new("/Users/test");
        for installed in [
            "/Applications/Removent.app/Contents/MacOS/removent",
            "/Users/test/Applications/Removent.app/Contents/MacOS/removent",
        ] {
            assert!(is_installed_app(Path::new(installed), home));
        }
        for development in [
            "/workspace/target/debug/removent",
            "/workspace/dist/Removent.app/Contents/MacOS/removent",
            "/Volumes/Removent/Removent.app/Contents/MacOS/removent",
            "/Applications/removent",
        ] {
            assert!(!is_installed_app(Path::new(development), home));
        }
    }
}

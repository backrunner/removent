use super::*;

impl Engine {
    pub fn set_system_startup(&self, on: bool) {
        let engine = self.clone();
        self.update_startup_settings(move |service| {
            engine.manage_system_startup(if on {
                "system-enable"
            } else {
                "system-disable"
            })?;
            if on {
                service.set_launch_at_login(true)?;
            }
            removent_core::settings::atomic_write(
                &engine.paths.run_dir().join("system-startup-configured"),
                if on { b"on\n" } else { b"off\n" },
            )?;
            Ok(())
        });
    }

    pub fn update_system_startup(&self) {
        let engine = self.clone();
        self.update_startup_settings(move |_| engine.manage_system_startup("system-update"));
    }

    pub(super) fn install_system_startup_defaults(&self) -> Result<()> {
        if removent_core::service_intent::is_stopped(&self.paths)? {
            return Ok(());
        }
        let exe = std::env::current_exe()?;
        let Some(home) = std::env::var_os("HOME") else {
            return Ok(());
        };
        if !is_installed_app(&exe, Path::new(&home)) {
            return Ok(());
        }
        let choice = self.paths.run_dir().join("system-startup-configured");
        let status = system_service::status()?;
        let previous = match std::fs::read_to_string(&choice) {
            Ok(value) => Some(value),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if !should_install(&status, previous.as_deref(), env!("CARGO_PKG_VERSION")) {
            return Ok(());
        }
        let bundle = system_service::installed_bundle(&exe)
            .ok_or_else(|| anyhow::anyhow!(t!("settings.system_signed_required").to_string()))?;
        // Development/local signing must never cause an administrator prompt.
        system_service::verify_bundle(&bundle)
            .map_err(|_| anyhow::anyhow!(t!("settings.system_signed_required").to_string()))?;
        // Cancellation is a pending setup, not permission to retry on every
        // launch. The visible settings action completes it when the user is ready.
        removent_core::settings::atomic_write(
            &choice,
            format!("pending:{}\n", env!("CARGO_PKG_VERSION")).as_bytes(),
        )?;
        self.manage_system_startup("system-enable")?;
        removent_core::settings::atomic_write(&choice, b"on\n")?;
        Ok(())
    }

    fn manage_system_startup(&self, action: &'static str) -> Result<()> {
        let exe = std::env::current_exe()?;
        let bundle = system_service::installed_bundle(&exe)
            .ok_or_else(|| anyhow::anyhow!(t!("settings.system_signed_required").to_string()))?;
        system_service::verify_bundle(&bundle)
            .map_err(|_| anyhow::anyhow!(t!("settings.system_signed_required").to_string()))?;
        let cli = bundle.join("Contents/MacOS/removent-cli");
        anyhow::ensure!(cli.is_file(), "{}", t!("settings.system_signed_required"));
        let output = std::process::Command::new("/usr/bin/osascript")
            .args(["-e", r#"on run argv
set cliPath to item 1 of argv
set dataPath to item 2 of argv
set serviceAction to item 3 of argv
do shell script "/usr/bin/env " & quoted form of ("REMOVENT_DATA_DIR=" & dataPath) & " " & quoted form of cliPath & " daemon " & quoted form of serviceAction & " --json" with administrator privileges
end run"#])
            .arg(cli).arg(&self.paths.root)
            .arg(action)
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Ok(())
    }
}

fn should_install(status: &SystemServiceStatus, choice: Option<&str>, version: &str) -> bool {
    if status.installed {
        status.enabled
            && status.version != version
            && choice != Some(format!("pending:{version}\n").as_str())
    } else {
        choice.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_enable_once_and_updates_preserve_disable_or_cancel() {
        let mut status = SystemServiceStatus::default();
        assert!(should_install(&status, None, "2"));
        for choice in ["pending:2\n", "off\n", "on\n"] {
            assert!(!should_install(&status, Some(choice), "2"));
        }
        status.installed = true;
        status.enabled = true;
        status.version = "1".into();
        assert!(should_install(&status, Some("on\n"), "2"));
        assert!(!should_install(&status, Some("pending:2\n"), "2"));
        assert!(should_install(&status, Some("pending:1\n"), "2"));
        status.enabled = false;
        assert!(!should_install(&status, None, "2"));
        status.enabled = true;
        status.version = "2".into();
        assert!(!should_install(&status, None, "2"));
    }
}

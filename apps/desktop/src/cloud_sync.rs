//! Start the provisioned CloudKit helper and observe its local storage results.
use crate::engine::{Engine, UiEvent};
use removent_client::{
    cloud_sync::{Command, dispatch},
    saved::SavedConnections,
};
use removent_core::DataPaths;
use serde_json::Value;
use std::time::Duration;

pub fn launch(paths: &DataPaths) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let contents = executable
        .parent()
        .and_then(|p| p.parent())
        .ok_or("App bundle unavailable")?;
    let helper = contents.join("Helpers/RemoventSync.app");
    if !helper.is_dir() {
        return Err("iCloud requires the packaged Removent app".into());
    }
    std::process::Command::new("/usr/bin/open")
        .args(["-g", "-n"])
        .arg(helper)
        .args(["--args", "--data-dir"])
        .arg(&paths.root)
        .arg("--parent-pid")
        .arg(std::process::id().to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|mut child| {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn start(engine: &Engine) {
    let paths = DataPaths {
        root: engine.data_dir(),
    };
    let events = engine.events_tx.clone();
    engine.rt.spawn(async move {
        let mut previous: Option<(Value, Vec<removent_client::saved::SavedConnection>)> = None;
        let mut last_launch = std::time::Instant::now() - Duration::from_secs(60);
        loop {
            let paths = paths.clone();
            let result =
                tokio::task::spawn_blocking(move || -> Result<_, removent_core::CoreError> {
                    let status = dispatch(&paths, Command::Status)?;
                    let entries = SavedConnections::load(&paths)?.all().to_vec();
                    Ok((paths, status, entries))
                })
                .await;
            if let Ok(Ok((paths, mut status, entries))) = result {
                if status["enabled"] == true
                    && last_launch.elapsed() >= Duration::from_secs(30)
                    && removent_core::DataDirLock::acquire_at(
                        &paths.root.join(".cloud-sync-owner.lock"),
                    )
                    .is_ok()
                {
                    last_launch = std::time::Instant::now();
                    if launch(&paths).is_err() {
                        status = dispatch(
                            &paths,
                            Command::Report {
                                scope: None,
                                code: "configuration".into(),
                            },
                        )
                        .unwrap_or(status);
                    }
                }
                if previous.as_ref() != Some(&(status.clone(), entries.clone())) {
                    previous = Some((status.clone(), entries.clone()));
                    if events.send(UiEvent::CloudSync { status, entries }).is_err() {
                        break;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

impl Engine {
    pub fn cloud_sync_command(&self, command: Command) -> Result<Value, String> {
        let launch_helper = matches!(command, Command::Enable { enabled: true } | Command::Retry);
        let paths = DataPaths {
            root: self.data_dir(),
        };
        let mut result = dispatch(&paths, command).map_err(|e| e.to_string())?;
        if launch_helper && result["enabled"] == true && launch(&paths).is_err() {
            result = dispatch(
                &paths,
                Command::Report {
                    scope: None,
                    code: "configuration".into(),
                },
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(result)
    }
}

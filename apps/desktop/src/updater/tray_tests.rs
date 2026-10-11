use super::*;
use std::process::{Child, Stdio};

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn fake_tray(bundle: &Path, paths: &DataPaths) -> Running {
    let executable = tray_bundle(bundle).join("Contents/MacOS/RemoventTray");
    std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
    if !executable.exists() {
        std::fs::copy("/bin/sleep", &executable).unwrap();
        assert!(
            Command::new("/usr/bin/codesign")
                .args(["--force", "--sign", "-"])
                .arg(&executable)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    paths.ensure_layout().unwrap();
    let child = Command::new("/usr/bin/python3")
        .arg("-c")
        .arg("import os,fcntl,sys; f=os.open(sys.argv[1],os.O_CREAT|os.O_RDWR,0o600); fcntl.flock(f,fcntl.LOCK_EX); os.set_inheritable(f,True); os.execv(sys.argv[2],[sys.argv[2],'60'])")
        .arg(paths.root.join(".tray.lock"))
        .arg(&executable)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn().unwrap();
    let running = Running(child);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !locked(paths).unwrap()
        || fingerprint(running.0.id() as i32, &executable)
            .unwrap()
            .is_none()
    {
        assert!(Instant::now() < deadline, "fake tray did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    running
}

#[test]
fn stopping_old_tray_waits_for_exit_and_preserves_other_data_directory() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    // Real process/lock coverage, including spaces in the executable path.
    let bundle = root.join("Update Test.app");
    let paths = DataPaths {
        root: root.join("first data"),
    };
    let unrelated = DataPaths {
        root: root.join("other data"),
    };
    let mut old = fake_tray(&bundle, &paths);
    let mut other = fake_tray(&bundle, &unrelated);
    assert!(stop_tray(&paths, &bundle).unwrap());
    old.0.wait().unwrap();
    assert!(!locked(&paths).unwrap());
    assert!(other.0.try_wait().unwrap().is_none());
    assert!(locked(&unrelated).unwrap());
    assert!(!stop_tray(&paths, &bundle).unwrap());
    // The replacement now acquires the same singleton lock.
    let mut replacement = fake_tray(&bundle, &paths);
    assert!(locked(&paths).unwrap());
    assert!(replacement.0.try_wait().unwrap().is_none());
}

#[test]
fn unknown_lock_holder_is_not_killed() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let paths = DataPaths {
        root: root.join("data"),
    };
    let mut tray = fake_tray(&root.join("Other.app"), &paths);
    assert!(stop_tray(&paths, &root.join("Removent.app")).is_err());
    assert!(tray.0.try_wait().unwrap().is_none());
    assert!(locked(&paths).unwrap());
}

#[test]
fn starting_missing_tray_reports_failure() {
    let directory = tempfile::tempdir().unwrap();
    let paths = DataPaths {
        root: directory.path().join("data"),
    };
    assert!(start_tray(&paths, &directory.path().join("Missing.app")).is_err());
}

#[test]
fn startup_replaces_tray_left_by_legacy_installer_even_after_backup_cleanup() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let paths = DataPaths {
        root: root.join("data"),
    };
    let bundle = root.join("Removent.app");
    let mut old = fake_tray(&bundle, &paths);
    assert!(
        !refresh_tray(&paths, &bundle).unwrap(),
        "current tray must survive reopening"
    );
    // Model the old installer: swap the app while its tray keeps running, then
    // render the new desktop's first frame and remove the old bundle backup.
    let backup = bundle.with_extension("app.old");
    std::fs::rename(&bundle, &backup).unwrap();
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::remove_dir_all(&backup).unwrap();
    assert!(refresh_tray(&paths, &bundle).unwrap());
    old.0.wait().unwrap();
    assert!(!locked(&paths).unwrap());
    let mut new = fake_tray(&bundle, &paths);
    assert!(!refresh_tray(&paths, &bundle).unwrap());
    assert!(new.0.try_wait().unwrap().is_none());
}

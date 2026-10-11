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
        // Do not re-sign a protected system executable (arm64e/platform code).
        // Build ordinary native code so CI exercises our process/lock handling
        // without depending on the runner's trust cache for /bin/sleep.
        let source = executable.with_extension("c");
        std::fs::write(
            &source,
            r#"#include <fcntl.h>
#include <stdio.h>
#include <sys/file.h>
#include <unistd.h>
int main(int argc, char **argv) {
    if (argc != 2) return 2;
    int fd = open(argv[1], O_CREAT | O_RDWR, 0600);
    if (fd < 0 || flock(fd, LOCK_EX | LOCK_NB) != 0) {
        perror("fixture lock");
        return 3;
    }
    for (;;) pause();
}
"#,
        )
        .unwrap();
        assert!(
            Command::new("/usr/bin/xcrun")
                .args(["clang", "-arch"])
                .arg(if cfg!(target_arch = "aarch64") {
                    "arm64"
                } else {
                    "x86_64"
                })
                .arg(&source)
                .arg("-o")
                .arg(&executable)
                .status()
                .unwrap()
                .success()
        );
        std::fs::remove_file(source).unwrap();
        let signing = Command::new("/usr/bin/codesign")
            .args([
                "--force",
                "--sign",
                "-",
                "--identifier",
                "com.alkinum.removent.qa.tray",
            ])
            .arg(&executable)
            .output()
            .unwrap();
        assert!(
            signing.status.success(),
            "fixture signing failed: {}",
            String::from_utf8_lossy(&signing.stderr)
        );
    }
    paths.ensure_layout().unwrap();
    let child = Command::new(&executable)
        .arg(paths.root.join(".tray.lock"))
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut running = Running(child);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !locked(paths).unwrap()
        || fingerprint(running.0.id() as i32, &executable)
            .unwrap()
            .is_none()
    {
        assert!(
            running.0.try_wait().unwrap().is_none(),
            "fake tray exited before acquiring its lock: {}",
            executable.display()
        );
        assert!(
            Instant::now() < deadline,
            "fake tray did not start: {}",
            executable.display()
        );
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

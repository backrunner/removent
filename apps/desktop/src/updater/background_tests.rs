//! Opt-in acceptance with a real packaged Swift tray and real launchd jobs.
use super::*;
use install::{replace_bundle, restart_background};
use removent_core::service::Service;
use std::os::unix::fs::PermissionsExt;
use tray::{start_tray, stop_tray};

fn fixture(bundle: &Path, version: &str) {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let tray = bundle.join("Contents/Helpers/RemoventTray.app");
    assert!(
        Command::new("/usr/bin/ditto")
            .arg(workspace.join("dist/RemoventTray.app"))
            .arg(&tray)
            .status()
            .unwrap()
            .success()
    );
    let macos = bundle.join("Contents/MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    // Use the real service CLI, so the packaged tray watchdog remains active.
    std::fs::copy(
        workspace.join("target/debug/removent-cli"),
        macos.join("removent-cli"),
    )
    .unwrap();
    let daemon = macos.join("removentd");
    let source = r#"#!/usr/bin/python3
import fcntl, json, os, pathlib, signal, socket, sys
root = pathlib.Path(os.environ['REMOVENT_DATA_DIR'])
lock = (root / '.daemon.lock').open('w')
fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
path = root / 'run/removentd.sock'
path.unlink(missing_ok=True)
server = socket.socket(socket.AF_UNIX)
server.bind(str(path))
server.listen()
(root / 'fixture-version').write_text('@VERSION@')
(root / 'fixture-pid').write_text(str(os.getpid()))
def stop(*args):
    path.unlink(missing_ok=True)
    sys.exit(0)
signal.signal(signal.SIGTERM, stop)
while True:
    client, _ = server.accept()
    try:
        request = client.recv(65536)
        if request:
            client.sendall(b'{"type":"error","message":"lifecycle fixture"}\n')
    except OSError:
        pass
    finally:
        client.close()
"#;
    std::fs::write(&daemon, source.replace("@VERSION@", version)).unwrap();
    std::fs::set_permissions(daemon, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
#[ignore = "requires a macOS GUI session, dist/RemoventTray.app and target/debug/removent-cli"]
fn packaged_update_restarts_tray_and_daemon_and_recovers_rollback() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let paths = DataPaths {
        root: root.join("data"),
    };
    paths.ensure_layout().unwrap();
    let bundle = root.join("Removent.app");
    let staged = root.join("Staged.app");
    fixture(&bundle, "old");
    fixture(&staged, "new");
    let service = Service::new(paths.clone(), bundle.join("Contents/MacOS/removentd")).unwrap();
    struct Cleanup<'a>(&'a Service, &'a Path);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = stop_tray(&self.0.paths, self.1);
            let _ = self.0.set_launch_at_login(false);
            let _ = self.0.stop();
        }
    }
    let _cleanup = Cleanup(&service, &bundle);
    let version = || std::fs::read_to_string(paths.root.join("fixture-version")).unwrap();
    let pid = || std::fs::read_to_string(paths.root.join("fixture-pid")).unwrap();

    restart_background(&bundle, &paths, true).unwrap();
    service.set_launch_at_login(true).unwrap();
    assert_eq!(version(), "old");
    let original_daemon = pid();
    let original_tray = tray::tray_process(&paths, &bundle).unwrap().unwrap().0;
    assert!(stop_tray(&paths, &bundle).unwrap());
    replace_bundle(&staged, &bundle).unwrap();
    restart_background(&bundle, &paths, true).unwrap();
    assert_eq!(version(), "new");
    assert_ne!(pid(), original_daemon);
    assert!(service.status().unwrap().launch_at_login);
    assert!(service.status().unwrap().reachable);
    assert_ne!(
        tray::tray_process(&paths, &bundle).unwrap().unwrap().0,
        original_tray
    );
    println!("PASS: old tray exits; packaged replacement tray starts; launchd runs the new daemon");

    // Exercise the same restore/restart sequence used when desktop relaunch fails.
    assert!(stop_tray(&paths, &bundle).unwrap());
    std::fs::rename(&bundle, root.join("Failed.app")).unwrap();
    std::fs::rename(bundle.with_extension("app.old"), &bundle).unwrap();
    restart_background(&bundle, &paths, true).unwrap();
    assert_eq!(version(), "old");
    println!("PASS: rollback restores both packaged tray and previous daemon");

    fixture(&staged, "new");
    let legacy_tray = tray::tray_process(&paths, &bundle).unwrap().unwrap().0;
    replace_bundle(&staged, &bundle).unwrap();
    // Existing released installers restart the daemon but leave the old tray.
    service.restart().unwrap();
    std::fs::remove_dir_all(bundle.with_extension("app.old")).unwrap();
    assert!(tray::refresh_tray(&paths, &bundle).unwrap());
    start_tray(&paths, &bundle).unwrap();
    assert_ne!(
        tray::tray_process(&paths, &bundle).unwrap().unwrap().0,
        legacy_tray
    );
    assert_eq!(version(), "new");
    assert!(!tray::refresh_tray(&paths, &bundle).unwrap());
    println!("PASS: new startup replaces tray left by the released installer after backup cleanup");

    service.stop().unwrap();
    assert!(stop_tray(&paths, &bundle).unwrap());
    // An explicitly stopped service stays stopped after the updated tray starts.
    restart_background(&bundle, &paths, false).unwrap();
    std::thread::sleep(Duration::from_secs(3));
    let status = service.status().unwrap();
    assert!(status.stopped_by_user && !status.reachable && !status.managed);
    assert!(status.launch_at_login);
    stop_tray(&paths, &bundle).unwrap();
    start_tray(&paths, &bundle).unwrap();
    assert!(!service.status().unwrap().reachable);
    println!("PASS: manual stop and login preference survive update and tray relaunch");
}

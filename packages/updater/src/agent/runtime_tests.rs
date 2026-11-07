//! Opt-in acceptance against signed local bundles. All jobs, data and copies
//! are isolated; no installed app, remote host, permissions or credentials change.
use super::*;
use removent_core::service::Service;
use std::{fs, process::Command};

struct Fixture {
    _temp: tempfile::TempDir,
    paths: DataPaths,
    bundle: PathBuf,
    signer: String,
}
impl Fixture {
    fn new(source: &Path) -> Self {
        let temp = tempfile::Builder::new()
            .prefix("rv-update-")
            .tempdir_in("/tmp")
            .unwrap();
        let paths = DataPaths {
            root: temp.path().join("data"),
        };
        paths.ensure_layout().unwrap();
        Settings {
            host_enabled: false,
            ..Default::default()
        }
        .save(&paths)
        .unwrap();
        io::private_dir(&registration::directory(&paths)).unwrap();
        let bundle = temp.path().join("Removent.app");
        copy(source, &bundle);
        let signer = std::env::var("REMOVENT_UPDATE_TEST_SIGNER")
            .expect("Set a Developer ID signing identity");
        version(&bundle, "0.0.1");
        sign(&bundle, &signer, None);
        Self {
            _temp: temp,
            paths,
            bundle,
            signer,
        }
    }
    fn service(&self) -> Service {
        Service::new(
            self.paths.clone(),
            self.bundle.join("Contents/MacOS/removentd"),
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.service().stop();
        let _ = io::remove(&removent_core::update_gate::bundle_marker(&self.bundle).unwrap());
        let _ = io::remove(&self.paths.run_dir().join("update-maintenance"));
    }
}
fn copy(from: &Path, to: &Path) {
    io::checked(
        Command::new("/usr/bin/ditto").arg(from).arg(to),
        Duration::from_secs(60),
    )
    .unwrap();
}
fn version(bundle: &Path, version: &str) {
    io::checked(
        Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", &format!("Set :RemoventReleaseVersion {version}")])
            .arg(bundle.join("Contents/Info.plist")),
        Duration::from_secs(10),
    )
    .unwrap();
}
fn sign(path: &Path, signer: &str, identifier: Option<&str>) {
    let mut command = Command::new("/usr/bin/codesign");
    command.args([
        "--force",
        "--options",
        "runtime",
        "--timestamp=none",
        "--sign",
        signer,
    ]);
    if let Some(identifier) = identifier {
        command.args(["--identifier", identifier]);
    }
    io::checked(command.arg(path), Duration::from_secs(30)).unwrap();
}
fn compile(source: &str, target: &Path) {
    let code = tempfile::Builder::new().suffix(".c").tempfile().unwrap();
    fs::write(code.path(), source).unwrap();
    io::checked(
        Command::new("/usr/bin/xcrun")
            .args(["clang", "-Os", "-mmacosx-version-min=26.0"])
            .arg(code.path())
            .arg("-o")
            .arg(target),
        Duration::from_secs(30),
    )
    .unwrap();
}
fn source() -> PathBuf {
    PathBuf::from(
        std::env::var_os("REMOVENT_UPDATE_TEST_BUNDLE").expect("Set a signed test bundle path"),
    )
    .canonicalize()
    .unwrap()
}

#[test]
#[ignore = "requires a GUI session, Developer ID identity and REMOVENT_UPDATE_TEST_BUNDLE; isolated launchd jobs only"]
fn failed_host_rolls_back_and_a_higher_repair_release_installs() {
    let source = source();
    let fixture = Fixture::new(&source);
    let policy =
        crate::UpdatePolicy::from_settings(&Settings::load_strict(&fixture.paths).unwrap());
    fixture.service().start().unwrap();
    let original = health::read(&fixture.paths).unwrap();
    let broken = fixture._temp.path().join("broken/Removent.app");
    copy(&source, &broken);
    version(&broken, "0.0.2");
    // Signed fault fixture: fake startup receipt, then an actual host process
    // crash. The repair candidate below uses the real packaged GUI and host.
    compile(
        r#"#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
int main(void) { char p[4096]; sleep(10); snprintf(p,sizeof(p),"%s/run/update-smoke-ok",getenv("REMOVENT_DATA_DIR")); FILE *f=fopen(p,"w"); if(!f)return 1; fputs("0.0.2",f); fclose(f); return 0; }
"#,
        &broken.join("Contents/MacOS/removent"),
    );
    compile(
        "int main(void) { return 42; }",
        &broken.join("Contents/Helpers/RemoventHost.app/Contents/MacOS/removentd"),
    );
    sign(
        &broken.join("Contents/MacOS/removent"),
        &fixture.signer,
        Some("com.alkinum.removent.desktop"),
    );
    sign(
        &broken.join("Contents/Helpers/RemoventHost.app"),
        &fixture.signer,
        None,
    );
    sign(&broken, &fixture.signer, None);
    // A permission-changing release must be rejected before the original PID
    // is stopped, even if every Apple signature is valid.
    io::checked(
        Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", "Add :NSTestUsageDescription string Needs consent"])
            .arg(broken.join("Contents/Info.plist")),
        Duration::from_secs(10),
    )
    .unwrap();
    sign(&broken, &fixture.signer, None);
    assert!(
        transaction::install(
            &fixture.paths,
            &fixture.bundle,
            &broken,
            "0.0.2",
            false,
            &policy
        )
        .is_err()
    );
    assert_eq!(health::read(&fixture.paths).unwrap().pid, original.pid);
    io::checked(
        Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", "Delete :NSTestUsageDescription"])
            .arg(broken.join("Contents/Info.plist")),
        Duration::from_secs(10),
    )
    .unwrap();
    sign(&broken, &fixture.signer, None);
    assert!(
        transaction::install(
            &fixture.paths,
            &fixture.bundle,
            &broken,
            "0.0.2",
            false,
            &policy
        )
        .is_err()
    );
    assert_eq!(crate::bundle_version(&fixture.bundle).unwrap(), "0.0.1");
    let recovered = health::read(&fixture.paths).unwrap();
    assert_ne!(recovered.pid, original.pid);
    assert_eq!(recovered.status.fp_short, original.status.fp_short);
    assert!(rejected(&fixture.paths, "0.0.2").unwrap());
    let repair = crate::bundle_version(&source).unwrap();
    assert!(!rejected(&fixture.paths, &repair).unwrap());
    transaction::install(
        &fixture.paths,
        &fixture.bundle,
        &source,
        &repair,
        false,
        &policy,
    )
    .unwrap();
    let healthy = health::read(&fixture.paths).unwrap();
    assert_eq!(healthy.version, repair);
    assert_eq!(healthy.status.fp_short, original.status.fp_short);
    assert!(fixture.bundle.with_extension("app.update-backup").exists());
    assert!(!fixture.paths.run_dir().join("update-maintenance").exists());
    println!(
        "PASS: permission preflight preserves PID; host crash rolls back; higher real repair survives 60-second probation"
    );
}

#[test]
#[ignore = "requires a GUI session, Developer ID identity and REMOVENT_UPDATE_TEST_BUNDLE; isolated launchd jobs only"]
fn stopped_host_stays_stopped_after_real_update_and_probe() {
    let source = source();
    let fixture = Fixture::new(&source);
    fixture.service().stop().unwrap();
    let policy =
        crate::UpdatePolicy::from_settings(&Settings::load_strict(&fixture.paths).unwrap());
    let version = crate::bundle_version(&source).unwrap();
    transaction::install(
        &fixture.paths,
        &fixture.bundle,
        &source,
        &version,
        false,
        &policy,
    )
    .unwrap();
    let status = fixture.service().status().unwrap();
    assert!(status.stopped_by_user);
    assert!(!status.reachable);
    assert!(!status.managed);
    assert_eq!(crate::bundle_version(&fixture.bundle).unwrap(), version);
    println!(
        "PASS: real desktop and isolated daemon probes complete without undoing the user's host stop"
    );
}

#[test]
#[ignore = "requires a GUI session, Developer ID identity and REMOVENT_UPDATE_TEST_BUNDLE; isolated launchd jobs only"]
fn rollback_restarts_host_even_if_final_journal_write_fails() {
    let source = source();
    let fixture = Fixture::new(&source);
    fixture.service().start().unwrap();
    let old = health::read(&fixture.paths).unwrap();
    let lease = removent_core::update_gate::MaintenanceLease::acquire(&fixture.paths).unwrap();
    fixture.service().quiesce_for_update(&lease).unwrap();
    let mut tx = journal::Journal::new(&fixture.bundle, "0.0.2", "0.0.1");
    tx.resume_host = true;
    fs::rename(&fixture.bundle, &tx.backup).unwrap();
    fs::write(
        fixture.paths.run_dir().join("update-maintenance"),
        "pending",
    )
    .unwrap();
    let unusable_journal = fixture._temp.path().join("unwritable-journal");
    fs::create_dir(&unusable_journal).unwrap();
    transaction::rollback(&fixture.paths, &mut tx, &unusable_journal, &lease).unwrap();
    let recovered = health::read(&fixture.paths).unwrap();
    assert_ne!(old.pid, recovered.pid);
    assert_eq!(old.status.fp_short, recovered.status.fp_short);
    assert!(!fixture.paths.run_dir().join("update-maintenance").exists());
    println!(
        "PASS: a failed final journal write does not prevent restoring and restarting the prior host"
    );
}

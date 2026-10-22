use super::*;
use crate::{PeersStore, identity, peers::PeerRecord};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

fn fixture(root: &Path) -> DataPaths {
    let paths = DataPaths {
        root: root.join("user"),
    };
    paths.ensure_layout().unwrap();
    identity::load_or_create(&paths, "Test Mac").unwrap();
    let settings = Settings {
        device_name: "Test Mac".into(),
        host_port: 48999,
        authentication: crate::authentication::AuthenticationSettings {
            mode: crate::AuthenticationMode::Password,
            password: "synthetic-private-password".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    settings.save(&paths).unwrap();
    let mut peers = PeersStore::load(&paths).unwrap();
    for (byte, trusted, input) in [("aa", true, true), ("bb", false, true), ("cc", true, false)] {
        peers
            .upsert(PeerRecord {
                fingerprint: byte.repeat(32),
                short_fp: byte.repeat(8),
                name: byte.into(),
                trusted,
                granted_caps: removent_proto::Caps {
                    input,
                    ..removent_proto::Caps::all()
                },
                added_at_unix: 1,
                last_connected_unix: 2,
            })
            .unwrap();
    }
    paths
}

#[test]
fn snapshot_keeps_identity_and_exact_video_input_grants_without_copying_app_passwords() {
    let dir = tempfile::tempdir().unwrap();
    let paths = fixture(dir.path());
    let staged = dir.path().join("staged");
    fs::create_dir(&staged).unwrap();
    let status = snapshot::stage(&paths, &staged).unwrap();
    let system = DataPaths {
        root: staged.join("data"),
    };
    assert_eq!(
        fs::read(system.device_key()).unwrap(),
        fs::read(paths.device_key()).unwrap()
    );
    assert_eq!(
        fs::read(system.device_cert()).unwrap(),
        fs::read(paths.device_cert()).unwrap()
    );
    assert_eq!(status.approved_devices.len(), 2);
    assert!(status.approved_devices[0].input);
    assert!(!status.approved_devices[1].input);
    let text = fs::read_to_string(system.settings_file()).unwrap();
    assert!(!text.contains("synthetic-private-password"));
    let settings = Settings::load_strict(&system).unwrap();
    assert_eq!(
        settings.authentication.mode,
        crate::AuthenticationMode::SystemAccount
    );
    assert!(settings.window_server_capture && settings.paired_only && settings.host_enabled);
    assert_eq!(settings.host_port, 48999);
    assert!(!settings.vnc_enabled && !settings.update_check_enabled);
    for peer in PeersStore::load(&system).unwrap().all() {
        assert!(peer.trusted && peer.granted_caps.video);
        assert!(
            !peer.granted_caps.audio && !peer.granted_caps.clipboard && !peer.granted_caps.file
        );
    }
    assert_eq!(
        fs::metadata(system.device_key())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(system.root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(staged.join("status.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
}

#[test]
fn mismatched_or_invalid_identity_is_rejected_before_creating_a_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let paths = fixture(dir.path());
    let other = tempfile::tempdir().unwrap();
    let other_paths = fixture(other.path());
    let staged = dir.path().join("staged");
    fs::create_dir(&staged).unwrap();
    for certificate in [
        fs::read(other_paths.device_cert()).unwrap(),
        b"invalid".to_vec(),
    ] {
        fs::write(paths.device_cert(), certificate).unwrap();
        assert!(snapshot::stage(&paths, &staged).is_err());
        assert_eq!(fs::read_dir(&staged).unwrap().count(), 0);
    }
}

#[test]
fn fresh_install_can_register_without_granting_unknown_controllers() {
    let dir = tempfile::tempdir().unwrap();
    let paths = fixture(dir.path());
    fs::remove_file(paths.settings_file()).unwrap();
    fs::remove_file(paths.peers_file()).unwrap();
    let staged = dir.path().join("staged");
    fs::create_dir(&staged).unwrap();
    assert!(
        snapshot::stage(&paths, &staged)
            .unwrap()
            .approved_devices
            .is_empty()
    );
    assert!(
        Settings::load_strict(&DataPaths {
            root: staged.join("data")
        })
        .unwrap()
        .paired_only
    );
}

#[test]
fn snapshot_rejects_symlinks_bad_peers_and_secret_bearing_parse_errors() {
    let dir = tempfile::tempdir().unwrap();
    let paths = fixture(dir.path());
    let link = dir.path().join("link");
    symlink(paths.device_key(), &link).unwrap();
    assert!(security::read_regular(&link).is_err());
    assert!(security::read_regular(dir.path()).is_err());
    let oversized = dir.path().join("large");
    fs::write(&oversized, vec![b'x'; 1_048_577]).unwrap();
    assert!(security::read_regular(&oversized).is_err());
    fs::write(paths.settings_file(), "password = 'never-print-me\n").unwrap();
    let staged = dir.path().join("staged");
    fs::create_dir(&staged).unwrap();
    let error = snapshot::stage(&paths, &staged).unwrap_err();
    assert!(!format!("{error:#}").contains("never-print-me"));
    let mut peers = PeersStore::load(&paths).unwrap().all().to_vec();
    peers[0].fingerprint = "invalid".into();
    fs::write(paths.peers_file(), serde_json::to_vec(&peers).unwrap()).unwrap();
    assert!(current_approved_devices(&paths).is_err());
}

#[test]
fn publication_replaces_complete_installation_and_rolls_back_failure() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("installed");
    let plist = dir.path().join("agent.plist");
    let staged = dir.path().join("staged");
    let stage = |value: &str, broken: bool| {
        fs::create_dir_all(staged.join("data/run")).unwrap();
        fs::write(staged.join("version"), value).unwrap();
        if broken {
            fs::create_dir(staged.join("data/run/service-stopped")).unwrap();
        } else {
            fs::write(staged.join("data/run/service-stopped"), "installing").unwrap();
        }
    };
    stage("first", false);
    install::publish(&staged, &root, &plist, true).unwrap();
    assert_eq!(fs::read_to_string(root.join("version")).unwrap(), "first");
    assert!(!root.join("data/run/service-stopped").exists());
    let original = fs::read(&plist).unwrap();
    stage("broken", true);
    assert!(install::publish(&staged, &root, &plist, true).is_err());
    assert_eq!(fs::read_to_string(root.join("version")).unwrap(), "first");
    assert_eq!(fs::read(&plist).unwrap(), original);
    stage("second", false);
    install::publish(&staged, &root, &plist, true).unwrap();
    assert_eq!(fs::read_to_string(root.join("version")).unwrap(), "second");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    stage("disabled-update", false);
    fs::remove_file(&plist).unwrap();
    install::publish(&staged, &root, &plist, false).unwrap();
    assert!(!plist.exists());
    assert!(root.join("data/run/service-stopped").exists());
}

#[test]
fn definition_targets_login_window_and_supports_stopping_without_respawn() {
    let xml = definition(Path::new("/Library/Removent & <Test>"));
    assert!(xml.contains("<string>LoginWindow</string>"));
    assert!(!xml.contains("<string>Aqua</string>"));
    assert!(xml.contains("--login-window"));
    assert!(xml.contains("data/run/service-stopped</key><false/>"));
    assert!(xml.contains("Removent &amp; &lt;Test&gt;"));
}

#[test]
fn developer_paths_and_unsigned_code_cannot_become_system_hosts() {
    let dir = tempfile::tempdir().unwrap();
    assert!(security::verify_root_owned(dir.path()).is_err());
    assert!(verify_bundle(dir.path()).is_err());
    assert!(installed_bundle(Path::new("/tmp/removent-cli")).is_none());
    assert_eq!(
        installed_bundle(Path::new(
            "/Applications/Removent.app/Contents/MacOS/removent-cli"
        )),
        Some(PathBuf::from("/Applications/Removent.app"))
    );
}

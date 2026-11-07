use super::*;

#[test]
fn defaults_enable_installation_but_preserve_an_existing_check_opt_out() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DataPaths {
        root: dir.path().into(),
    };
    assert!(Settings::load_strict(&paths).unwrap().update_auto_install);
    std::fs::write(paths.settings_file(), "update_check_enabled = false\n").unwrap();
    let old = Settings::load_strict(&paths).unwrap();
    assert!(!old.update_check_enabled);
    assert!(old.update_auto_install);
    Settings::update(&paths, |s| s.update_auto_install = false).unwrap();
    assert!(!Settings::load_strict(&paths).unwrap().update_auto_install);
}

#[test]
fn a_bad_release_does_not_block_a_higher_repair_release_or_erase_opt_out() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DataPaths {
        root: dir.path().into(),
    };
    io::private_dir(&registration::directory(&paths)).unwrap();
    reject_version(&paths, "1.2.0").unwrap();
    reject_version(&paths, "1.2.0").unwrap();
    assert!(rejected(&paths, "1.2.0").unwrap());
    assert!(!rejected(&paths, "1.2.1").unwrap());
    assert!(crate::manifest::eligible_upgrade(
        removent_core::UpdateChannel::Stable,
        "1.2.1",
        "1.1.0"
    ));
}

#[test]
fn unsafe_or_corrupt_state_is_never_treated_as_permission_to_install() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DataPaths {
        root: dir.path().into(),
    };
    std::fs::write(paths.settings_file(), "update_auto_install = [broken").unwrap();
    assert!(Settings::load_strict(&paths).is_err());
    assert!(paths.settings_file().exists());
    let target = dir.path().join("real.json");
    io::save(&target, &true).unwrap();
    let link = dir.path().join("link.json");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(io::load::<bool>(&link).is_err());
    std::fs::write(&target, b"partial").unwrap();
    assert!(io::load::<bool>(&target).is_err());
}

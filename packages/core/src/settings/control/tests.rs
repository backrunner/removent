use super::*;

fn paths() -> (tempfile::TempDir, DataPaths) {
    let dir = tempfile::tempdir().unwrap();
    let paths = DataPaths {
        root: dir.path().into(),
    };
    (dir, paths)
}

#[test]
fn catalog_covers_every_persisted_leaf_and_defaults_parse() {
    fn leaves(value: &Value, prefix: &str, keys: &mut Vec<String>) {
        if let Value::Object(map) = value {
            for (key, value) in map {
                let key = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                leaves(value, &key, keys);
            }
        } else {
            keys.push(prefix.into());
        }
    }
    let settings = Settings::default();
    let mut keys = Vec::new();
    leaves(&serde_json::to_value(&settings).unwrap(), "", &mut keys);
    keys.sort();
    let mut catalog: Vec<_> = SETTINGS
        .iter()
        .map(|setting| setting.key.to_owned())
        .collect();
    catalog.sort();
    assert_eq!(keys, catalog);
    for setting in SETTINGS {
        let value = read_value(&settings, setting.key);
        let input = value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| value.to_string());
        assert_eq!(setting.parse(&input).unwrap(), value, "{}", setting.key);
    }
}

#[test]
fn rejected_edits_leave_the_original_bytes_and_credentials_intact() {
    let (_dir, paths) = paths();
    let mut initial = Settings::default();
    initial.authentication.password = "private-password".into();
    initial.authentication.mode = crate::AuthenticationMode::Password;
    initial.save(&paths).unwrap();
    let before = std::fs::read(paths.settings_file()).unwrap();
    for (key, input) in [
        ("host_port", Some("0")),
        ("vnc_port", Some("65536")),
        ("theme", Some("unknown")),
        ("device_name", Some("  ")),
        ("device_name", Some("abc\nlog injection")),
        ("host_enabled", Some("perhaps")),
        ("authentication.password", None),
        ("authentication.mode", Some("otp")),
        ("authentication.otp_secret", Some("private-invalid-secret")),
        (
            "update_endpoint",
            Some("http://example.invalid/latest.json"),
        ),
        (
            "update_endpoint",
            Some("https://user:secret@example.invalid/latest.json"),
        ),
    ] {
        let error = edit(&paths, key, input).unwrap_err();
        assert!(!error.to_string().contains("private-"));
        assert_eq!(std::fs::read(paths.settings_file()).unwrap(), before);
    }
}

#[test]
fn credentials_are_redacted_in_all_views_and_valid_modes_can_be_configured() {
    let (_dir, paths) = paths();
    edit(&paths, "authentication.password", Some("private-password")).unwrap();
    edit(&paths, "authentication.mode", Some("password")).unwrap();
    let otp = crate::AuthenticationSettings::generate_otp_secret();
    edit(&paths, "authentication.otp_secret", Some(&otp)).unwrap();
    edit(&paths, "authentication.mode", Some("otp")).unwrap();
    let settings = edit(&paths, "vnc_password", Some("private-vnc-password")).unwrap();
    let views: Vec<_> = SETTINGS
        .iter()
        .map(|setting| setting.view(&settings))
        .collect();
    let output = serde_json::to_string(&views).unwrap();
    assert!(!output.contains("private-password"));
    assert!(!output.contains("private-vnc-password"));
    assert!(!output.contains(&otp));
    assert_eq!(
        find("authentication.password").unwrap().view(&settings)["value"]["configured"],
        true
    );
    edit(&paths, "authentication.mode", None).unwrap();
    let settings = edit(&paths, "authentication.password", None).unwrap();
    assert!(settings.authentication.password.is_empty());
}

#[test]
fn cli_edits_share_the_desktop_and_daemon_lock() {
    let (_dir, paths) = paths();
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            barrier.wait();
            Settings::update(&paths, |s| s.host_enabled = false).unwrap();
        });
        scope.spawn(|| {
            barrier.wait();
            edit(&paths, "device_name", Some("Office Mac")).unwrap();
        });
    });
    let settings = Settings::load_strict(&paths).unwrap();
    assert!(!settings.host_enabled);
    assert_eq!(settings.device_name, "Office Mac");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(paths.settings_file())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn malformed_settings_are_not_moved_reset_or_echoed() {
    let (_dir, paths) = paths();
    let original = "vnc_password = \"private-unclosed-password";
    std::fs::write(paths.settings_file(), original).unwrap();
    for error in [
        Settings::load_strict(&paths).unwrap_err().to_string(),
        edit(&paths, "device_name", Some("Name"))
            .unwrap_err()
            .to_string(),
    ] {
        assert!(!error.contains("private-unclosed-password"));
    }
    assert_eq!(
        std::fs::read_to_string(paths.settings_file()).unwrap(),
        original
    );
    assert!(!paths.settings_file().with_extension("toml.bak").exists());
}

#[test]
fn enum_aliases_and_host_effects_match_the_catalog() {
    assert_eq!(find("language").unwrap().parse("zh-CN").unwrap(), "ZhCn");
    assert_eq!(
        find("video_quality")
            .unwrap()
            .parse("high-quality")
            .unwrap(),
        "HighQuality"
    );
    let current = Settings::default();
    let mut next = current.clone();
    next.theme = super::super::Theme::Dark;
    next.discovery.rdp = true;
    next.update_channel = super::super::UpdateChannel::Beta;
    assert!(!current.prevent_idle_sleep);
    next.prevent_idle_sleep = true;
    assert_eq!(
        find("prevent_idle_sleep").unwrap().view(&next)["apply"],
        "daemon_reload"
    );
    assert!(!host_changed(&current, &next));
    next.host_port += 1;
    assert!(host_changed(&current, &next));
}

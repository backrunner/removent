use super::manifest::signature_payload;
use super::*;
use ed25519_dalek::{Signer, SigningKey};
use std::os::unix::process::ExitStatusExt;

fn fetch_output(code: i32, body: &[u8], stderr: &str) -> std::process::Output {
    std::process::Output {
        status: std::process::ExitStatus::from_raw(code << 8),
        stdout: body.to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

#[test]
fn fetch_preserves_manifest_body_without_status_metadata() {
    let body = b"{\"version\":\"0.1.0\"}\n";
    assert_eq!(
        decode_fetch_output(fetch_output(0, body, "\n200")).unwrap(),
        body
    );
}

#[test]
fn fetch_distinguishes_http_failures_with_the_same_curl_exit_code() {
    for (status, retry) in [(403, false), (404, false), (429, true), (503, true)] {
        let error = decode_fetch_output(fetch_output(
            22,
            b"",
            &format!("curl: (22) The requested URL returned error: {status}\n\n{status}"),
        ))
        .unwrap_err();
        assert_eq!(error.http_status, Some(status));
        assert_eq!(error.is_transient(), retry);
        assert!(error.user_message().contains(&format!("HTTP {status}")));
    }
}

#[test]
fn fetch_retries_interrupted_transfers_but_rejects_certificate_failures() {
    let interrupted = decode_fetch_output(fetch_output(
        18,
        b"{\"version\":",
        "curl: (18) transfer closed with outstanding read data remaining\n\n200",
    ))
    .unwrap_err();
    assert!(interrupted.is_transient());
    assert_eq!(interrupted.http_status, Some(200));
    let timeout =
        decode_fetch_output(fetch_output(28, b"", "curl: (28) timeout\n\n000")).unwrap_err();
    assert!(timeout.is_transient());
    assert_eq!(timeout.http_status, None);
    let certificate = decode_fetch_output(fetch_output(
        60,
        b"",
        "curl: (60) SSL certificate problem\n\n000",
    ))
    .unwrap_err();
    assert!(!certificate.is_transient());
}

#[test]
fn retry_discards_partial_body_and_uses_the_remaining_time_budget() {
    let mut budgets = Vec::new();
    let body = fetch_with_retry(
        "https://example.com",
        Duration::from_secs(30),
        |_, budget| {
            budgets.push(budget);
            decode_fetch_output(if budgets.len() == 1 {
                fetch_output(18, b"{\"version\":", "curl: (18) interrupted\n\n200")
            } else {
                fetch_output(0, b"{\"version\":\"0.1.0\"}", "\n200")
            })
        },
    )
    .unwrap();
    assert_eq!(body, "{\"version\":\"0.1.0\"}");
    assert_eq!(budgets.len(), 2);
    assert!(budgets[1] < budgets[0]);
    assert!(budgets[0] <= Duration::from_secs(30));
}

#[test]
fn retry_stops_on_permanent_failure_or_exhausted_budget() {
    for (status, budget) in [
        (404, Duration::from_secs(30)),
        (503, Duration::from_millis(20)),
    ] {
        let mut attempts = 0;
        let error = fetch_with_retry("https://example.com", budget, |_, _| {
            attempts += 1;
            decode_fetch_output(fetch_output(22, b"", &format!("\n{status}")))
        })
        .unwrap_err();
        assert_eq!(attempts, 1);
        assert!(error.contains(&format!("HTTP {status}")));
    }
}

#[test]
fn release_requirement_is_accepted_as_inline_source() {
    let result = Command::new("/usr/bin/csreq")
        .args(["-r", &release_codesign_requirement(), "-t"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn manifest() -> UpdateManifest {
    UpdateManifest {
        version: "0.1.1".into(),
        url: "https://example.com/Removent-0.1.1-macos-arm64.zip".into(),
        sha256: "ab12".into(),
        notes: "notes".into(),
        pub_date: "2026-08-23T00:00:00Z".into(),
        min_compatible_proto: 1,
        signature: String::new(),
    }
}

#[test]
fn version_parse_and_compare() {
    assert_eq!(parse_version("1.2.3"), Some(semver::Version::new(1, 2, 3)));
    assert_eq!(
        parse_version("v0.10.2"),
        Some(semver::Version::new(0, 10, 2))
    );
    assert_eq!(
        parse_version(" 1.0.0 "),
        Some(semver::Version::new(1, 0, 0))
    );
    assert_eq!(parse_version("1.2"), None);
    assert_eq!(parse_version("1.2.3.4"), None);
    assert_eq!(parse_version("a.b.c"), None);

    assert!(is_newer("0.1.1", "0.1.0"));
    assert!(is_newer("0.2.0", "0.1.9"));
    assert!(is_newer("1.0.0", "0.9.9"));
    assert!(!is_newer("0.1.0", "0.1.0"));
    assert!(!is_newer("0.1.0", "0.1.1"));
    assert!(!is_newer("garbage", "0.1.0"));
    assert!(is_newer("9.0.0-preview.1", "0.1.0"));
    assert!(!is_newer("0.1.0+build2", "0.1.0+build1"));
    assert!(is_newer("0.1.3", "0.1.3-beta.1"));
    assert!(!is_newer("0.1.2", "0.1.3-beta.1"));
    assert!(is_newer("0.1.3-beta.2", "0.1.3-beta.1"));
}

#[test]
fn switching_channels_never_changes_semver_upgrade_direction() {
    use UpdateChannel::{Beta, Stable};
    for (remote, local, stable, beta) in [
        ("0.1.3-beta.1", "0.1.2", false, true),
        ("0.1.3-beta.2", "0.1.3-beta.1", false, true),
        ("0.1.3-beta.10", "0.1.3-beta.2", false, true),
        ("0.1.3-beta.2", "0.1.3-beta.10", false, false),
        ("0.1.2", "0.1.3-beta.1", false, false),
        ("0.1.3", "0.1.3-beta.2", true, true),
        ("0.1.3-beta.3", "0.1.3", false, false),
        ("0.1.3", "0.2.0-beta.1", false, false),
        ("0.2.0-beta.1", "0.1.3", false, true),
        ("0.1.3-beta.1", "0.1.3-beta.1", false, false),
        ("0.1.3+two", "0.1.3+one", false, false),
        ("0.1.3-beta.2+two", "0.1.3-beta.2+one", false, false),
        ("0.1.3", "0.1.3", false, false),
        ("9.0.0-alpha.1", "0.1.3", false, false),
        ("9.0.0-rc.1", "0.1.3", false, false),
        ("9.0.0-beta.custom", "0.1.3", false, false),
        ("9.0.0", "invalid", false, false),
        ("invalid", "0.1.3", false, false),
    ] {
        assert_eq!(
            eligible_upgrade(Stable, remote, local),
            stable,
            "stable {local} -> {remote}"
        );
        assert_eq!(
            eligible_upgrade(Beta, remote, local),
            beta,
            "beta {local} -> {remote}"
        );
    }
}

fn release(version: &str, prerelease: bool) -> serde_json::Value {
    serde_json::json!({
        "tag_name": format!("v{version}"), "draft": false, "prerelease": prerelease,
        "assets": [{"name": "latest.json", "browser_download_url": format!("https://github.com/backrunner/removent/releases/download/v{version}/latest.json")}]
    })
}

fn signed_manifest(version: &str, signing: &SigningKey) -> String {
    let mut m = manifest();
    m.version = version.into();
    m.sha256 = "ab".repeat(32);
    m.signature = hex::encode(signing.sign(&signature_payload(&m)).to_bytes());
    serde_json::json!({"version":m.version,"url":m.url,"sha256":m.sha256,
        "min_compatible_proto":m.min_compatible_proto,"signature":m.signature})
    .to_string()
}

#[test]
fn beta_discovery_filters_drafts_other_prereleases_and_untrusted_asset_urls() {
    let mut draft = release("9.0.0", false);
    draft["draft"] = true.into();
    let mut missing = release("8.0.0", false);
    missing["assets"] = serde_json::json!([]);
    let mut wrong_url = release("7.0.0", false);
    wrong_url["assets"][0]["browser_download_url"] = "https://example.com/latest.json".into();
    let entries = vec![
        release("0.1.3-beta.2", true),
        release("0.1.3", false),
        release("0.1.3-beta.10", true),
        draft,
        missing,
        wrong_url,
        release("6.0.0-alpha.1", true),
        release("5.0.0", true),
        release("4.0.0-beta.1", false),
    ];
    let releases: Vec<GithubRelease> = serde_json::from_value(serde_json::json!(entries)).unwrap();
    assert_eq!(
        release_candidate(&releases, "0.1.2").unwrap().0.to_string(),
        "0.1.3"
    );
    assert!(release_candidate(&releases, "0.1.3").is_none());
}

#[test]
fn beta_fetch_paginates_and_verifies_the_selected_manifest() {
    let signing = SigningKey::from_bytes(&[7u8; 32]);
    let policy = UpdatePolicy {
        channel: UpdateChannel::Beta,
        endpoint: String::new(),
    };
    let mut requests = vec![];
    let found = fetch_update(&policy, "0.1.2", &signing.verifying_key(), |url| {
        requests.push(url.to_string());
        if url.ends_with("page=1") {
            Ok(serde_json::json!(vec![release("0.1.3-beta.2", true); 100]).to_string())
        } else if url.ends_with("page=2") {
            Ok(serde_json::json!([release("0.1.3-beta.10", true)]).to_string())
        } else {
            assert!(url.ends_with("/v0.1.3-beta.10/latest.json"));
            Ok(signed_manifest("0.1.3-beta.10", &signing))
        }
    })
    .unwrap()
    .unwrap();
    assert_eq!(found.version, "0.1.3-beta.10");
    assert_eq!(requests.len(), 3);
}

#[test]
fn signed_mirrors_still_obey_channel_and_no_downgrade_policy() {
    let signing = SigningKey::from_bytes(&[7u8; 32]);
    for (channel, remote, local, expected) in [
        (UpdateChannel::Stable, "0.1.3-beta.2", "0.1.2", false),
        (UpdateChannel::Beta, "0.1.3-beta.2", "0.1.2", true),
        (UpdateChannel::Beta, "0.1.3-beta.2", "0.1.3", false),
        (UpdateChannel::Stable, "0.1.2", "0.1.3-beta.2", false),
        (UpdateChannel::Stable, "0.1.3", "0.1.3-beta.2", true),
    ] {
        let policy = UpdatePolicy {
            channel,
            endpoint: "https://mirror.example/latest.json".into(),
        };
        let found = fetch_update(&policy, local, &signing.verifying_key(), |url| {
            assert_eq!(url, policy.endpoint);
            Ok(signed_manifest(remote, &signing))
        })
        .unwrap();
        assert_eq!(found.is_some(), expected);
    }
}

#[test]
fn beta_rejects_invalid_signatures_or_signed_version_mismatching_the_tag() {
    let signing = SigningKey::from_bytes(&[7u8; 32]);
    let policy = UpdatePolicy {
        channel: UpdateChannel::Beta,
        endpoint: String::new(),
    };
    for body in [
        signed_manifest("0.1.4-beta.1", &signing),
        signed_manifest("0.1.3-beta.2", &SigningKey::from_bytes(&[8u8; 32])),
    ] {
        assert!(
            fetch_update(&policy, "0.1.2", &signing.verifying_key(), |url| {
                Ok(if url.starts_with(RELEASES_API) {
                    serde_json::json!([release("0.1.3-beta.2", true)]).to_string()
                } else {
                    body.clone()
                })
            })
            .is_err()
        );
    }
}

#[test]
fn channel_change_invalidates_candidates_and_staged_installs_but_not_busy_work() {
    let shared = UpdateShared::new(&Settings::default());
    let mut state = shared.lock().unwrap();
    for status in [
        UpdateStatus::Available {
            version: "2.0.0".into(),
            notes: String::new(),
        },
        UpdateStatus::ReadyToInstall {
            version: "2.0.0".into(),
        },
    ] {
        state.status = status;
        state.manifest = Some(manifest());
        state.staged_app = Some(PathBuf::from("/staged/Removent.app"));
        let next = if state.policy.channel == UpdateChannel::Stable {
            UpdateChannel::Beta
        } else {
            UpdateChannel::Stable
        };
        state
            .change_policy(UpdatePolicy {
                channel: next,
                endpoint: String::new(),
            })
            .unwrap();
        assert_eq!(state.status, UpdateStatus::Idle);
        assert!(state.manifest.is_none());
        assert!(state.staged_app.is_none());
    }
    for status in [
        UpdateStatus::Checking,
        UpdateStatus::Downloading {
            version: "2.0.0".into(),
        },
        UpdateStatus::Verifying {
            version: "2.0.0".into(),
        },
        UpdateStatus::Swapping {
            version: "2.0.0".into(),
        },
        UpdateStatus::Relaunching,
    ] {
        state.status = status.clone();
        assert!(
            state
                .change_policy(UpdatePolicy {
                    channel: UpdateChannel::Beta,
                    endpoint: String::new()
                })
                .is_err()
        );
        assert_eq!(state.policy.channel, UpdateChannel::Stable);
        assert_eq!(state.status, status);
    }
}

#[test]
fn stale_download_and_install_requests_cannot_downgrade() {
    for version in ["0.0.1", env!("CARGO_PKG_VERSION")] {
        let shared = UpdateShared::new(&Settings::default());
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let mut s = shared.lock().unwrap();
            let mut m = manifest();
            m.version = version.into();
            s.manifest = Some(m);
            s.status = UpdateStatus::Available {
                version: version.into(),
                notes: String::new(),
            };
        }
        run_download(0, test_paths(), shared.clone(), tx.clone());
        assert!(matches!(
            rx.try_recv().unwrap(),
            UiEvent::UpdateStatus(UpdateStatus::Failed(_))
        ));
        {
            let mut s = shared.lock().unwrap();
            s.status = UpdateStatus::ReadyToInstall {
                version: version.into(),
            };
            s.staged_app = Some(PathBuf::from("/nonexistent/Removent.app"));
        }
        run_install(0, shared, tx, Arc::new(Mutex::new(None)));
        assert!(matches!(
            rx.try_recv().unwrap(),
            UiEvent::UpdateStatus(UpdateStatus::Failed(_))
        ));
    }
}

#[test]
fn queued_actions_do_not_survive_switching_channels_and_back() {
    let shared = UpdateShared::new(&Settings::default());
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let mut s = shared.lock().unwrap();
        for channel in [UpdateChannel::Beta, UpdateChannel::Stable] {
            s.change_policy(UpdatePolicy {
                channel,
                endpoint: String::new(),
            })
            .unwrap();
        }
        let mut m = manifest();
        m.version = "9.0.0".into();
        s.manifest = Some(m);
        s.status = UpdateStatus::Available {
            version: "9.0.0".into(),
            notes: String::new(),
        };
    }
    // A click queued against generation 0 must not download a replacement
    // candidate that appeared after switching back to the same channel.
    run_download(0, test_paths(), shared.clone(), tx.clone());
    assert!(matches!(
        shared.lock().unwrap().status,
        UpdateStatus::Available { .. }
    ));
    {
        let mut s = shared.lock().unwrap();
        s.status = UpdateStatus::ReadyToInstall {
            version: "9.0.0".into(),
        };
        s.staged_app = Some(PathBuf::from("/nonexistent/Removent.app"));
    }
    run_install(0, shared.clone(), tx, Arc::new(Mutex::new(None)));
    assert!(matches!(
        shared.lock().unwrap().status,
        UpdateStatus::ReadyToInstall { .. }
    ));
    assert!(rx.try_recv().is_err());
}

#[test]
#[ignore = "Read-only HTTPS check against the published GitHub beta feed"]
fn live_beta_feed_has_signed_upgrade_from_previous_stable() {
    let m = fetch_update(
        &UpdatePolicy {
            channel: UpdateChannel::Beta,
            endpoint: String::new(),
        },
        "0.1.2",
        &release_verifying_key().unwrap(),
        curl_get,
    )
    .unwrap()
    .unwrap();
    assert!(eligible_upgrade(UpdateChannel::Beta, &m.version, "0.1.2"));
}

#[test]
fn staging_failure_preserves_installed_bundle_and_backup() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("Removent.app");
    let backup = bundle.with_extension("app.old");
    std::fs::create_dir(&bundle).unwrap();
    std::fs::write(bundle.join("version"), "current").unwrap();
    std::fs::create_dir(&backup).unwrap();
    assert!(replace_bundle(&dir.path().join("missing.app"), &bundle).is_err());
    assert_eq!(
        std::fs::read_to_string(bundle.join("version")).unwrap(),
        "current"
    );
    assert!(backup.exists());
}

#[test]
fn successful_swap_keeps_previous_version_for_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("Removent.app");
    let staged = dir.path().join("stage.app");
    for (path, version) in [(&bundle, "old"), (&staged, "new")] {
        std::fs::create_dir(path).unwrap();
        std::fs::write(path.join("version"), version).unwrap();
    }
    replace_bundle(&staged, &bundle).unwrap();
    assert_eq!(
        std::fs::read_to_string(bundle.join("version")).unwrap(),
        "new"
    );
    assert_eq!(
        std::fs::read_to_string(bundle.with_extension("app.old").join("version")).unwrap(),
        "old"
    );
    assert!(!staged.exists());
}

#[test]
fn signature_payload_format() {
    // Contract: four \n-separated lines, no trailing newline.
    let payload = signature_payload(&manifest());
    assert_eq!(
        payload,
        b"0.1.1\nhttps://example.com/Removent-0.1.1-macos-arm64.zip\nab12\n1"
    );
}

#[test]
fn signature_verify_positive_and_negative() {
    let signing = SigningKey::from_bytes(&[7u8; 32]);
    let key = signing.verifying_key();

    let mut m = manifest();
    let sig = signing.sign(&signature_payload(&m));
    m.signature = hex::encode(sig.to_bytes());
    assert!(verify_manifest_signature(&m, &key));

    // Tampered payload (different sha256 after signing).
    let mut tampered = m.clone();
    tampered.sha256 = "ff00".into();
    assert!(!verify_manifest_signature(&tampered, &key));

    // Wrong key.
    let other = SigningKey::from_bytes(&[9u8; 32]).verifying_key();
    assert!(!verify_manifest_signature(&m, &other));

    // Malformed signature encodings.
    let mut bad = m.clone();
    bad.signature = "not-hex".into();
    assert!(!verify_manifest_signature(&bad, &key));
    bad.signature = hex::encode([0u8; 32]); // wrong length
    assert!(!verify_manifest_signature(&bad, &key));
}

#[test]
fn release_public_key_constant_parses() {
    assert!(release_verifying_key().is_some());
}

// ---- state-machine concurrency guards ----

fn test_paths() -> DataPaths {
    DataPaths {
        root: std::env::temp_dir().join("removent-updater-test"),
    }
}

#[test]
fn run_download_ignores_non_available_state() {
    // A second click while a download is already running must be a no-op
    // (and must not touch the network or the .part file).
    let shared = UpdateShared::new(&Settings::default());
    shared.lock().unwrap().status = UpdateStatus::Downloading {
        version: "0.1.1".into(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    run_download(0, test_paths(), shared.clone(), tx);
    assert_eq!(
        shared.lock().unwrap().status,
        UpdateStatus::Downloading {
            version: "0.1.1".into()
        }
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn manual_check_keeps_ready_to_install() {
    // A staged update must survive even a manual re-check; overwriting it
    // would orphan the staged bundle and force a re-download.
    let shared = UpdateShared::new(&Settings::default());
    shared.lock().unwrap().status = UpdateStatus::ReadyToInstall {
        version: "0.1.1".into(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    shared.lock().unwrap().policy.endpoint = "http://127.0.0.1:1/latest.json".into();
    run_check(true, shared.clone(), tx);
    assert_eq!(
        shared.lock().unwrap().status,
        UpdateStatus::ReadyToInstall {
            version: "0.1.1".into()
        }
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn run_install_ignores_non_ready_state() {
    let shared = UpdateShared::new(&Settings::default());
    shared.lock().unwrap().status = UpdateStatus::Downloading {
        version: "0.1.1".into(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    run_install(0, shared.clone(), tx, Arc::new(Mutex::new(None)));
    assert_eq!(
        shared.lock().unwrap().status,
        UpdateStatus::Downloading {
            version: "0.1.1".into()
        }
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn run_install_claims_swap_atomically() {
    // ReadyToInstall + staged bundle claims the Swapping transition; the
    // swap itself fails in a dev environment (not running from a .app),
    // landing in Failed — but a concurrent second call sees Swapping and
    // returns instead of racing swap_bundle.
    let shared = UpdateShared::new(&Settings::default());
    {
        let mut s = shared.lock().unwrap();
        s.status = UpdateStatus::ReadyToInstall {
            version: "9.0.0".into(),
        };
        let mut m = manifest();
        m.version = "9.0.0".into();
        s.manifest = Some(m);
        s.staged_app = Some(PathBuf::from("/nonexistent/Removent.app"));
    }
    let (tx, rx) = std::sync::mpsc::channel();
    run_install(0, shared.clone(), tx, Arc::new(Mutex::new(None)));
    assert!(matches!(
        rx.try_recv().unwrap(),
        UiEvent::UpdateStatus(UpdateStatus::Swapping { .. })
    ));
    match shared.lock().unwrap().status.clone() {
        UpdateStatus::Failed(_) => {}
        other => panic!("expected Failed after dev-env swap, got {other:?}"),
    }
}

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

struct Cli {
    root: tempfile::TempDir,
}
impl Cli {
    fn new() -> Self {
        let root = tempfile::tempdir_in("/tmp").unwrap();
        std::fs::write(
            root.path().join("settings.toml"),
            "language = \"En\"\nhost_enabled = false\n",
        )
        .unwrap();
        Self { root }
    }
    fn run(&self, args: &[&str], input: Option<&str>) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_removent-cli"))
            .args(args)
            .env("REMOVENT_DATA_DIR", self.root.path())
            .env_remove("RUST_LOG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        drop(child.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() > deadline {
                child.kill().unwrap();
                let _ = child.wait();
                panic!("CLI hung");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.wait_with_output().unwrap()
    }
    fn file(&self) -> String {
        std::fs::read_to_string(self.root.path().join("settings.toml")).unwrap()
    }
}
fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[test]
fn idle_sleep_setting_defaults_off_and_supports_set_get_reset() {
    let cli = Cli::new();
    let get = || cli.run(&["settings", "get", "prevent_idle_sleep", "--json"], None);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&stdout(&get())).unwrap()["value"],
        false
    );
    let set = cli.run(
        &["settings", "set", "prevent_idle_sleep", "true", "--json"],
        None,
    );
    assert!(
        set.status.success(),
        "{}",
        String::from_utf8_lossy(&set.stderr)
    );
    let value: serde_json::Value = serde_json::from_str(&stdout(&set)).unwrap();
    assert_eq!(value["application"], "daemon_offline");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&stdout(&get())).unwrap()["value"],
        true
    );
    assert!(
        cli.run(&["settings", "reset", "prevent_idle_sleep", "--json"], None)
            .status
            .success()
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&stdout(&get())).unwrap()["value"],
        false
    );
}
fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}
fn success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        stdout(output),
        stderr(output)
    );
    assert!(output.stderr.is_empty(), "{}", stderr(output));
}

#[test]
fn offline_settings_support_discovery_edit_get_reset_and_machine_output() {
    let cli = Cli::new();
    let output = cli.run(&["settings", "list", "--json"], None);
    success(&output);
    let all: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        all.as_array().unwrap().len(),
        removent_core::settings::control::SETTINGS.len()
    );
    for args in [
        vec!["settings", "set", "device_name", "Office Mac"],
        vec!["settings", "set", "discovery.vnc", "true"],
        vec!["settings", "set", "video_quality", "high-quality"],
        vec!["settings", "set", "theme", "dark"],
        vec!["settings", "set", "update_channel", "beta"],
    ] {
        success(&cli.run(&args, None));
    }
    let output = cli.run(&["settings", "get", "device_name"], None);
    success(&output);
    assert_eq!(stdout(&output), "Office Mac\n");
    let output = cli.run(&["settings", "get", "discovery.vnc", "--json"], None);
    success(&output);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["value"],
        true
    );
    let output = cli.run(&["settings", "reset", "discovery.vnc", "--json"], None);
    success(&output);
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["setting"]["value"], false);
    assert_eq!(result["application"], "desktop_restart");
    let output = cli.run(&["settings", "set", "host_port", "49777", "--json"], None);
    success(&output);
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["application"], "daemon_offline");
    assert_eq!(result["saved"], true);
    assert!(!cli.root.path().join("run/removentd.sock").exists());
    let path = cli.run(&["settings", "path", "--json"], None);
    success(&path);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&path.stdout).unwrap()["path"],
        cli.root.path().join("settings.toml").to_str().unwrap()
    );
}

#[test]
fn secrets_only_enter_via_stdin_and_never_appear_in_output_or_logs() {
    let cli = Cli::new();
    let secret = "test-private-password";
    let output = cli.run(
        &["settings", "set", "authentication.password", secret],
        None,
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(!stderr(&output).contains(secret));
    let output = cli.run(
        &[
            "settings",
            "set",
            "authentication.password",
            "--stdin",
            "--json",
        ],
        Some(&format!("{secret}\n")),
    );
    success(&output);
    assert!(!stdout(&output).contains(secret));
    assert!(cli.file().contains(secret));
    success(&cli.run(
        &["settings", "set", "authentication.mode", "password"],
        None,
    ));
    for args in [
        &["settings", "get", "--json"][..],
        &["settings", "list"],
        &["settings", "get", "authentication.password", "--json"],
    ] {
        let output = cli.run(args, None);
        success(&output);
        assert!(!stdout(&output).contains(secret));
    }
    let before = cli.file();
    let output = cli.run(&["settings", "reset", "authentication.password"], None);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(cli.file(), before);
    let log = std::fs::read_to_string(cli.root.path().join("logs/removent-cli.log")).unwrap();
    assert!(!log.contains(secret));
}

#[test]
fn invalid_values_and_corrupt_files_do_not_reset_the_configuration() {
    let cli = Cli::new();
    let before = cli.file();
    for args in [
        &["settings", "set", "host_port", "0"][..],
        &["settings", "set", "host_enabled", "maybe"],
        &["settings", "reset"],
        &["settings", "set", "unknown", "1"],
        &["settings", "set", "theme", "blue"],
        &["settings", "set", "device_name", "Office", "extra"],
    ] {
        let output = cli.run(args, None);
        assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
        assert!(stderr(&output).contains("settings --help"));
        assert_eq!(cli.file(), before);
    }
    let malformed = "vnc_password = \"private-unclosed-value";
    std::fs::write(cli.root.path().join("settings.toml"), malformed).unwrap();
    for args in [
        &["settings", "get", "--json"][..],
        &["settings", "set", "theme", "dark", "--verbose"],
    ] {
        let output = cli.run(args, None);
        assert_eq!(output.status.code(), Some(1));
        assert!(!stderr(&output).contains("private-unclosed-value"));
        assert_eq!(cli.file(), malformed);
        assert!(!cli.root.path().join("settings.toml.bak").exists());
    }
}

#[test]
fn persisted_changes_and_reload_failures_are_reported_separately() {
    for response in [
        "{\"type\":\"ok\"}\n",
        "{\"type\":\"error\",\"message\":\"Reload refused\"}\n",
    ] {
        let cli = Cli::new();
        std::fs::create_dir(cli.root.path().join("run")).unwrap();
        let listener = UnixListener::bind(cli.root.path().join("run/removentd.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = String::new();
            BufReader::new(socket.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&request).unwrap()["type"],
                "reload_settings"
            );
            socket.write_all(response.as_bytes()).unwrap();
        });
        let output = cli.run(
            &["settings", "set", "device_name", "Reloaded Mac", "--json"],
            None,
        );
        server.join().unwrap();
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["saved"], true);
        assert!(cli.file().contains("Reloaded Mac"));
        if response.contains("refused") {
            assert_eq!(output.status.code(), Some(1));
            assert_eq!(result["application"], "reload_failed");
            assert!(stderr(&output).contains("Settings were saved"));
            assert!(stderr(&output).contains("daemon reload"));
        } else {
            success(&output);
            assert_eq!(result["application"], "reload_requested");
        }
    }
}

#[test]
fn setting_values_that_look_like_global_flags_are_preserved() {
    let cli = Cli::new();
    for value in ["--verbose", "--json", "-v"] {
        success(&cli.run(&["settings", "set", "device_name", value], None));
        assert_eq!(
            stdout(&cli.run(&["settings", "get", "device_name"], None)),
            format!("{value}\n")
        );
        let output = cli.run(&["settings", "--json", "set", "device_name", value], None);
        success(&output);
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["setting"]["value"], value);
    }
}

//! Exercise the actual CLI's stdout, stderr, exit codes and IPC boundary.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

struct Cli {
    root: tempfile::TempDir,
}

impl Cli {
    fn new(language: &str) -> Self {
        // Unix sockets have a short path limit on macOS.
        let root = tempfile::tempdir_in("/tmp").unwrap();
        std::fs::write(
            root.path().join("settings.toml"),
            format!("language = {language:?}\n"),
        )
        .unwrap();
        Self { root }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_removent-cli"));
        command
            .args(args)
            .env("REMOVENT_DATA_DIR", self.root.path())
            .env_remove("RUST_LOG")
            .env_remove("RUST_BACKTRACE")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut child = self.command(args).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(12);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!("CLI hung for {args:?}: {}", stderr(&output));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.wait_with_output().unwrap()
    }

    fn listener(&self) -> UnixListener {
        std::fs::create_dir_all(self.root.path().join("run")).unwrap();
        let listener = UnixListener::bind(self.root.path().join("run/removentd.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        listener
    }

    fn serve(&self, response: &'static str, delay: Duration) -> std::thread::JoinHandle<String> {
        let listener = self.listener();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(12);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    Err(error) => panic!("CLI did not connect: {error}"),
                }
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = String::new();
            BufReader::new(socket.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            std::thread::sleep(delay);
            let _ = socket.write_all(response.as_bytes());
            request
        })
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn missing_and_stale_daemons_have_actionable_localized_errors() {
    for (language, expected) in [
        ("En", "background service is currently unavailable"),
        ("ZhCn", "后台服务当前不可用"),
    ] {
        let cli = Cli::new(language);
        for stale in [false, true] {
            if stale {
                drop(cli.listener());
            }
            for args in [
                &["daemon", "status"][..],
                &["pairing", "show"],
                &["pairing", "watch"],
            ] {
                let output = cli.run(args);
                assert_eq!(output.status.code(), Some(1));
                assert!(output.stdout.is_empty());
                let error = stderr(&output);
                assert!(error.contains(expected), "{error}");
                assert!(error.contains("daemon start"), "{error}");
                assert!(error.contains("--verbose"));
                for hidden in [
                    "INFO",
                    "logging:",
                    "os error",
                    "Caused by:",
                    "removentd.sock",
                ] {
                    assert!(!error.contains(hidden), "{error}");
                }
            }
        }
        let log = std::fs::read_to_string(cli.root.path().join("logs/removent-cli.log")).unwrap();
        assert!(log.contains("local diagnostics initialized"));
        assert!(!cli.root.path().join("identity").exists());
    }
}

#[test]
fn verbose_explicitly_enables_diagnostics_without_changing_exit_status() {
    let cli = Cli::new("En");
    for args in [
        &["--verbose", "daemon", "status"][..],
        &["daemon", "status", "-v"],
    ] {
        let output = cli.run(args);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error = stderr(&output);
        assert!(error.contains("local diagnostics initialized"), "{error}");
        assert!(error.contains("os error 2"), "{error}");
        assert!(error.contains("daemon start"));
    }
    let output = cli
        .command(&["daemon", "status"])
        .env("RUST_LOG", "trace")
        .output()
        .unwrap();
    assert!(!stderr(&output).contains("local diagnostics initialized"));
}

#[test]
fn help_version_and_invalid_arguments_do_not_touch_the_daemon_or_logs() {
    let cli = Cli::new("En");
    for args in [
        &[][..],
        &["--help"],
        &["--verbose", "--help"],
        &["--version"],
        &["daemon", "--help"],
        &["pairing", "-h"],
        &["ping", "--help"],
        &["identity", "--help"],
    ] {
        let output = cli.run(args);
        assert!(output.status.success(), "{args:?}: {}", stderr(&output));
        assert!(output.stderr.is_empty());
        assert!(!output.stdout.is_empty());
    }
    for args in [
        &["wat"][..],
        &["daemon", "wat"],
        &["daemon", "start", "--typo"],
        &["pairing", "wat"],
        &["pairing", "generate", "extra"],
        &["identity", "extra"],
        &["ping"],
        &["ping", "localhost", "--count", "0"],
        &["ping", "localhost", "--timeout", "0"],
        &["ping", "localhost", "--count", "oops"],
        &["ping", "localhost", "--pin"],
    ] {
        let output = cli.run(args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            stderr(&output)
        );
        assert!(output.stdout.is_empty());
        assert!(stderr(&output).contains("--help"));
        assert!(!stderr(&output).contains("unavailable"));
    }
    assert!(!cli.root.path().join("logs").exists());
    assert!(!cli.root.path().join("run").exists());
}

#[test]
fn successful_json_is_clean_even_when_stdout_and_stderr_are_combined() {
    let cli = Cli::new("En");
    let server = cli.serve("{\"type\":\"pairing_pin\",\"pin\":\"123456\"}\n{\"type\":\"status\",\"running\":false,\"host_ready\":false}\n", Duration::ZERO);
    let output = cli.run(&["daemon", "status"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(output.stderr.is_empty());
    let combined = [output.stdout, output.stderr].concat();
    let value: serde_json::Value = serde_json::from_slice(&combined).unwrap();
    assert_eq!(value["type"], "status");
    assert_eq!(value["running"], false);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&server.join().unwrap()).unwrap()["type"],
        "status"
    );
    let log = std::fs::read_to_string(cli.root.path().join("logs/removent-cli.log")).unwrap();
    assert!(!log.contains("123456"));
}

#[test]
fn permission_errors_explain_access_instead_of_suggesting_a_start() {
    use std::os::unix::fs::PermissionsExt;
    let cli = Cli::new("En");
    let _listener = cli.listener();
    let run_dir = cli.root.path().join("run");
    std::fs::set_permissions(&run_dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    // A privileged test runner may bypass directory permissions.
    let denied = std::fs::read_dir(&run_dir).is_err();
    let output = cli.run(&["daemon", "status"]);
    std::fs::set_permissions(&run_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    if denied {
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error = stderr(&output);
        assert!(error.contains("Permission denied"), "{error}");
        assert!(error.contains("data directory permissions"));
        assert!(!error.contains("daemon start"));
    }
}

#[cfg(target_os = "macos")]
#[test]
fn service_status_remains_parseable_by_the_tray_without_starting_a_daemon() {
    let cli = Cli::new("En");
    let output = cli.run(&["daemon", "service-status"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(output.stderr.is_empty());
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for key in ["reachable", "managed", "launch_at_login", "stopped_by_user"] {
        assert_eq!(status[key], false, "{status}");
    }
    assert!(!cli.root.path().join("run/removentd.sock").exists());
}

#[test]
fn response_failures_and_timeouts_are_not_reported_as_a_stopped_service() {
    for (response, delay, expected) in [
        ("", Duration::ZERO, "disconnected"),
        ("bad json\n", Duration::ZERO, "Cannot read"),
        (
            "{\"type\":\"error\",\"message\":\"Sharing is disabled\"}\n",
            Duration::ZERO,
            "Sharing is disabled",
        ),
        (
            "",
            Duration::from_secs(6),
            "did not respond within 5 seconds",
        ),
    ] {
        let cli = Cli::new("En");
        let server = cli.serve(response, delay);
        let output = cli.run(&["daemon", "status"]);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error = stderr(&output);
        assert!(error.contains(expected), "{error}");
        assert!(error.contains("daemon service-status"));
        assert!(!error.contains("daemon start"));
        assert!(!error.contains("Caused by:"));
        server.join().unwrap();
    }
}

#[test]
fn pairing_watch_keeps_listening_when_no_code_is_active() {
    let cli = Cli::new("En");
    let server = cli.serve("{\"type\":\"error\",\"message\":\"No active code\"}\n{\"type\":\"pairing_pin\",\"pin\":\"123456\"}\n", Duration::ZERO);
    let output = cli.run(&["pairing", "watch"]);
    // EOF is reported after delivering the event, not silently treated as success.
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["pin"],
        "123456"
    );
    assert!(stderr(&output).contains("disconnected"));
    server.join().unwrap();
}

#[test]
fn cloud_sync_retains_its_json_protocol_without_creating_logs() {
    let cli = Cli::new("En");
    let mut child = cli
        .command(&["cloud-sync"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"type\":\"not-a-command\"}")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["ok"],
        false
    );
    assert!(!cli.root.path().join("logs").exists());
    assert!(!cli.root.path().join("identity").exists());
}

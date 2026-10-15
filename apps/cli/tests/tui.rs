//! Exercise keyboard input and terminal behavior against the compiled CLI.
#[test]
fn inline_terminal_acceptance() {
    let output = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/qa/test_cli_tui.py"
        ))
        .arg(env!("CARGO_BIN_EXE_removent-cli"))
        .output()
        .expect("Python 3 is required for the PTY acceptance suite");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

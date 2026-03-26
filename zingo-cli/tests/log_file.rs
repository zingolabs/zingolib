use std::io::Write;
use std::process::{Command, Stdio};

/// Launches zingo-cli in interactive mode with a local-only wallet
/// (seed + birthday, nosync) and verifies that the tracing subscriber
/// writes to the log file and not to stderr.
///
/// Uses RUST_LOG=info so that startup info messages are emitted,
/// providing observable output to verify the redirect.
#[test]
fn interactive_mode_redirects_tracing_to_log_file() {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let log_path = tmp.path().join("cli.log");
    let data_dir = tmp.path().join("wallets");

    let mut child = Command::new(env!("CARGO_BIN_EXE_zingo-cli"))
        .env("RUST_LOG", "info")
        .arg("--server")
        .arg("https://127.0.0.1:1")
        .arg("--data-dir")
        .arg(&data_dir)
        .arg("--log-file")
        .arg(&log_path)
        .arg("--nosync")
        .arg("--seed")
        .arg("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art")
        .arg("--birthday")
        .arg("600000")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn zingo-cli");

    // Give the process a moment to start, then quit.
    std::thread::sleep(std::time::Duration::from_secs(3));
    if let Some(ref mut stdin) = child.stdin {
        let _ = writeln!(stdin, "quit");
    }

    let output = child.wait_with_output().expect("failed to wait on child");

    // The log file should contain the startup info message.
    let log_contents = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        log_contents.contains("Starting Zingo-CLI"),
        "Expected 'Starting Zingo-CLI' in log file at {}, but got:\n{log_contents}",
        log_path.display()
    );

    // Stderr should be empty — tracing went to the file, not stderr.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.is_empty(),
        "Expected empty stderr (tracing should go to log file), but got:\n{stderr}"
    );
}

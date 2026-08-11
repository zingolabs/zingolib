#![allow(clippy::disallowed_methods)]

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Resolves the `zingo-cli` binary path.
///
/// First tries the compile-time path from `CARGO_BIN_EXE_zingo-cli`.
/// If that doesn't exist (e.g. nextest archive on a different machine),
/// falls back to locating it relative to the current test binary,
/// which is in `target/debug/deps/` — the `zingo-cli` binary is in
/// `target/debug/`.
fn zingo_cli_binary() -> PathBuf {
    let compile_time = PathBuf::from(env!("CARGO_BIN_EXE_zingo-cli"));
    if compile_time.exists() {
        return compile_time;
    }
    // Fallback: resolve relative to the test binary location.
    let test_exe = std::env::current_exe().expect("current_exe");
    let deps_dir = test_exe.parent().expect("deps dir");
    let target_dir = deps_dir.parent().expect("target dir");
    let candidate = target_dir.join("zingo-cli");
    assert!(
        candidate.exists(),
        "Could not find zingo-cli binary at {} or {}",
        compile_time.display(),
        candidate.display()
    );
    candidate
}

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

    // No consent act is passed, so every build shape launches offline:
    // the session skips the mixnet driver, needs no proxy, and still
    // writes its startup lines — this test observes log redirection only.
    let mut command = Command::new(zingo_cli_binary());
    let mut child = command
        .env("RUST_LOG", "info")
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

    // Wait for the startup INFO lines to reach the log file, polling
    // instead of sleeping a fixed interval; the ceiling only bounds the
    // pathological case.
    let deadline = std::time::Instant::now() + zingo_netutils::time::test::LOG_FLUSH_DEADLINE;
    loop {
        if std::fs::read_to_string(&log_path)
            .unwrap_or_default()
            .contains("INFO")
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no INFO line reached the log file within {:?}",
            zingo_netutils::time::test::LOG_FLUSH_DEADLINE
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if let Some(ref mut stdin) = child.stdin {
        let _ = writeln!(stdin, "quit");
    }

    let output = child.wait_with_output().expect("failed to wait on child");

    let log_contents = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        log_contents.contains("Starting Zingo-CLI"),
        "Expected 'Starting Zingo-CLI' in log file at {}, but got:\n{log_contents}",
        log_path.display()
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    for level in ["INFO", "WARN", "ERROR", "DEBUG", "TRACE"] {
        assert!(
            !stderr.contains(&format!(" {level} ")),
            "Tracing {level} lines should go to the log file, not stderr \
             (stderr carries only Narration, ADR 0031). Got:\n{stderr}"
        );
    }
}

/// The error string that pepper_sync's `#[instrument(err)]` on
/// `get_latest_block` logs when the gRPC call fails.
#[cfg(feature = "nym")]
const EXPECTED_ERROR: &str = "pepper_sync::client::fetch";

/// Starts a mock gRPC server where all methods return `DEADLINE_EXCEEDED`.
/// The `#[instrument(err)]` on pepper_sync's `get_latest_block` emits a
/// tracing ERROR when sync calls it and gets the error back.
///
/// Verifies:
/// - The log file contains `ERROR` and the specific error message
/// - stderr does NOT contain formatted tracing ERROR lines
///
/// The scenario names a server and syncs against it — online acts, which
/// the offline-only build refuses at launch (ADR 0026) — so the test
/// exists only with the mixnet capability compiled in.
#[cfg(feature = "nym")]
#[tokio::test]
async fn tracing_error_from_pepper_sync_goes_to_log_file() {
    use zingo_grpc_proxy::tonic_reexport as tonic;
    use zingo_grpc_proxy::{CompactTxStreamerServer, ConfigurableMockStreamer, MockConfig};

    let config = MockConfig::all_error(tonic::Code::DeadlineExceeded, EXPECTED_ERROR);
    let svc = CompactTxStreamerServer::new(ConfigurableMockStreamer::new(config));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock server");
    let port = listener.local_addr().expect("local addr").port();
    let server_uri = format!("http://127.0.0.1:{port}");

    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(svc)
            .serve_with_incoming(incoming)
            .await
            .ok();
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let tmp = tempfile::tempdir().expect("create temp dir");
    let log_path = tmp.path().join("cli.log");
    let data_dir = tmp.path().join("wallets");
    let stub_proxy = write_stub_proxy(tmp.path());

    let mut child = Command::new(zingo_cli_binary())
        .env("RUST_LOG", "info")
        .arg("--server")
        .arg(&server_uri)
        // The mixnet is unconditional for a connected session, so hand the
        // spawner a stub that answers the directory query and then exits at
        // once: the session goes online, the bootstrap dies, and the
        // clearnet sync — the sole clearnet exception — still runs against
        // the mock to produce the ERROR.
        .arg("--nym-proxy")
        .arg(&stub_proxy)
        .arg("--data-dir")
        .arg(&data_dir)
        .arg("--log-file")
        .arg(&log_path)
        .arg("--seed")
        .arg("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art")
        .arg("--birthday")
        .arg("600000")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn zingo-cli");

    // Wait for the tracing ERROR to reach the log file, polling instead of
    // sleeping a fixed interval. The mock answers immediately with its
    // configured error status, so the ERROR lands as soon as the child's
    // first fetch call completes — typically well under a second after
    // startup. (Awaiting tokio::time::sleep between polls is what makes
    // that true: this test runs on the default current-thread runtime, so
    // a blocking wait here would starve the spawned mock server, leave the
    // child's request unanswered, and make the child sit out its full
    // 10-second client-side RPC timeout instead.) The ceiling only bounds
    // the pathological case.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let log_contents = std::fs::read_to_string(&log_path).unwrap_or_default();
        if log_contents.contains("ERROR") && log_contents.contains(EXPECTED_ERROR) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no tracing ERROR containing '{EXPECTED_ERROR}' reached the log \
             file within 20s.\nLog contents:\n{log_contents}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    if let Some(ref mut stdin) = child.stdin {
        let _ = writeln!(stdin, "quit");
    }

    let output = child.wait_with_output().expect("failed to wait on child");

    let log_contents = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        log_contents.contains("ERROR") && log_contents.contains(EXPECTED_ERROR),
        "Expected tracing ERROR with '{EXPECTED_ERROR}' in log file at {}.\nLog contents:\n{log_contents}",
        log_path.display()
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains(" ERROR "),
        "Tracing errors should go to the log file, not stderr. Got:\n{stderr}"
    );
}

/// Writes a stub `nym-proxy` into `dir`: it answers `--discover` with one
/// Exit Node so the session can draw a Clutch, and exits at once otherwise
/// so the bootstrap dies exactly as this test intends.
#[cfg(feature = "nym")]
fn write_stub_proxy(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("stub-nym-proxy");
    std::fs::write(
        &path,
        "#!/bin/sh\nfor arg in \"$@\"; do\n  if [ \"$arg\" = --discover ]; then\n    echo \"NYM_EXIT=stub-exit-node\"\n  fi\ndone\nexit 0\n",
    )
    .expect("write the stub proxy");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("make the stub proxy executable");
    }
    path
}

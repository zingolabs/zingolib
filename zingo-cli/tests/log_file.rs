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

    std::thread::sleep(std::time::Duration::from_secs(3));
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
    assert!(
        stderr.is_empty(),
        "Expected empty stderr (tracing should go to log file), but got:\n{stderr}"
    );
}

/// The specific error string pepper_sync emits via `#[instrument(err)]`
/// on `get_tree_state` when the call returns `DEADLINE_EXCEEDED`.
const EXPECTED_ERROR: &str = "get_tree_state timeout";

/// Starts a mock gRPC server where `get_latest_block` returns a valid
/// response (so sync can start) but `get_tree_state` returns
/// `DEADLINE_EXCEEDED` with [`EXPECTED_ERROR`].
///
/// pepper_sync's `#[instrument(err)]` on `get_tree_state` emits a tracing
/// ERROR which must appear in the log file — not on stderr.
#[tokio::test]
async fn tracing_error_from_pepper_sync_goes_to_log_file() {
    use zingo_grpc_proxy::tonic_reexport as tonic;
    use zingo_grpc_proxy::{
        CompactTxStreamerServer, ConfigurableMockStreamer, MethodBehavior, MethodHandler, MockConfig,
    };

    // All methods return DEADLINE_EXCEEDED by default.
    // Override get_latest_block to return a valid response so sync starts
    // and eventually calls get_tree_state (which still errors).
    let config = MockConfig::all_error(tonic::Code::DeadlineExceeded, EXPECTED_ERROR)
        .with_get_latest_block(MethodHandler::from_fn(|_req| async {
            Ok(tonic::Response::new(
                zingo_grpc_proxy::service::BlockId {
                    height: 600100,
                    hash: vec![],
                },
            ))
        }));

    let svc = CompactTxStreamerServer::new(ConfigurableMockStreamer::new(config));

    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        l.local_addr().expect("local addr").port()
    };
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().expect("parse addr");
    let server_uri = format!("http://127.0.0.1:{port}");

    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(svc)
            .serve(addr)
            .await
            .ok();
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let tmp = tempfile::tempdir().expect("create temp dir");
    let log_path = tmp.path().join("cli.log");
    let data_dir = tmp.path().join("wallets");

    let mut child = Command::new(env!("CARGO_BIN_EXE_zingo-cli"))
        .env("RUST_LOG", "error")
        .arg("--server")
        .arg(&server_uri)
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

    // Give sync time to progress and hit the get_tree_state error.
    std::thread::sleep(std::time::Duration::from_secs(8));

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

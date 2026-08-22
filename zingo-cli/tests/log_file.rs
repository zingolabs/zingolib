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
    let socks5_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake SOCKS5");
    let socks5_addr = socks5_listener.local_addr().expect("local addr");
    tokio::spawn(serve_provable_socks5(socks5_listener));
    let stub_proxy = write_stub_proxy(tmp.path(), socks5_addr);

    let mut child = Command::new(zingo_cli_binary())
        .env("RUST_LOG", "info")
        .arg("--server")
        .arg(&server_uri)
        // The mixnet is unconditional for a connected session, and the
        // blocking enable fails closed on a client that cannot prove, so
        // the stub announces the test-hosted SOCKS5 endpoint above: the
        // Sentinel's round trip gets bytes back, the standing client is
        // born proven, and the clearnet sync — the sole clearnet
        // exception — runs against the mock to produce the ERROR.
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

/// The stem of the Exit Node identities the stub proxy discovers, numbered
/// so a boot draws a distinct exit for every lane.
#[cfg(feature = "nym")]
const STUB_EXIT_NODE: &str = "stub-exit-node";

/// How many Exit Nodes the stub directory advertises, an agreeing
/// representation of `zingolib`'s `QUARTET_SIZE`, because a boot draws one
/// exit per role and refuses outright when the directory offers fewer.
#[cfg(feature = "nym")]
const STUB_EXIT_COUNT: usize = 4;

/// The SOCKS5 protocol version byte.
#[cfg(feature = "nym")]
const SOCKS5_VERSION: u8 = 0x05;

/// The SOCKS5 method byte for no authentication.
#[cfg(feature = "nym")]
const SOCKS5_NO_AUTH: u8 = 0x00;

/// The SOCKS5 reply byte for a succeeded request.
#[cfg(feature = "nym")]
const SOCKS5_SUCCEEDED: u8 = 0x00;

/// The SOCKS5 address-type byte for an IPv4 address.
#[cfg(feature = "nym")]
const SOCKS5_ATYP_IPV4: u8 = 0x01;

/// The SOCKS5 address-type byte for a domain name.
#[cfg(feature = "nym")]
const SOCKS5_ATYP_DOMAIN: u8 = 0x03;

/// The SOCKS5 address-type byte for an IPv6 address.
#[cfg(feature = "nym")]
const SOCKS5_ATYP_IPV6: u8 = 0x04;

/// The octet count of an IPv4 address on the SOCKS5 wire.
#[cfg(feature = "nym")]
const IPV4_OCTETS: usize = 4;

/// The octet count of an IPv6 address on the SOCKS5 wire.
#[cfg(feature = "nym")]
const IPV6_OCTETS: usize = 16;

/// The byte count of a port on the SOCKS5 wire.
#[cfg(feature = "nym")]
const PORT_BYTES: usize = 2;

/// The SOCKS5 reserved byte, always zero.
#[cfg(feature = "nym")]
const SOCKS5_RESERVED: u8 = 0x00;

/// The byte count of a client greeting header: version plus method count.
#[cfg(feature = "nym")]
const SOCKS5_GREETING_BYTES: usize = 2;

/// The byte count of a request header: version, command, reserved, and
/// address type.
#[cfg(feature = "nym")]
const SOCKS5_REQUEST_HEADER_BYTES: usize = 4;

/// The byte count of a domain-name length prefix.
#[cfg(feature = "nym")]
const DOMAIN_LEN_BYTES: usize = 1;

/// The unspecified IPv4 bind address and port a reply may carry.
#[cfg(feature = "nym")]
const SOCKS5_UNSPECIFIED_BIND: [u8; IPV4_OCTETS + PORT_BYTES] = [0; IPV4_OCTETS + PORT_BYTES];

/// The buffer size for the tunneled payload the fake endpoint echoes.
#[cfg(feature = "nym")]
const ECHO_BUFFER_BYTES: usize = 64;

/// Serves a minimal SOCKS5 endpoint that accepts any CONNECT and echoes the
/// first tunneled payload back, so the Sentinel's round trip reads bytes and
/// the probing birth proves its exit.
#[cfg(feature = "nym")]
async fn serve_provable_socks5(listener: tokio::net::TcpListener) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(async move {
            let mut greeting = [0u8; SOCKS5_GREETING_BYTES];
            if stream.read_exact(&mut greeting).await.is_err() {
                return;
            }
            let mut methods = vec![0u8; greeting[1] as usize];
            if stream.read_exact(&mut methods).await.is_err() {
                return;
            }
            if stream
                .write_all(&[SOCKS5_VERSION, SOCKS5_NO_AUTH])
                .await
                .is_err()
            {
                return;
            }
            let mut request = [0u8; SOCKS5_REQUEST_HEADER_BYTES];
            if stream.read_exact(&mut request).await.is_err() {
                return;
            }
            let address_octets = match request[SOCKS5_REQUEST_HEADER_BYTES - 1] {
                SOCKS5_ATYP_IPV4 => IPV4_OCTETS,
                SOCKS5_ATYP_IPV6 => IPV6_OCTETS,
                SOCKS5_ATYP_DOMAIN => {
                    let mut len = [0u8; DOMAIN_LEN_BYTES];
                    if stream.read_exact(&mut len).await.is_err() {
                        return;
                    }
                    len[0] as usize
                }
                _ => return,
            };
            let mut remainder = vec![0u8; address_octets + PORT_BYTES];
            if stream.read_exact(&mut remainder).await.is_err() {
                return;
            }
            let mut reply = vec![
                SOCKS5_VERSION,
                SOCKS5_SUCCEEDED,
                SOCKS5_RESERVED,
                SOCKS5_ATYP_IPV4,
            ];
            reply.extend_from_slice(&SOCKS5_UNSPECIFIED_BIND);
            if stream.write_all(&reply).await.is_err() {
                return;
            }
            let mut payload = [0u8; ECHO_BUFFER_BYTES];
            let Ok(read) = stream.read(&mut payload).await else {
                return;
            };
            if read == 0 {
                return;
            }
            let _ = stream.write_all(&payload[..read]).await;
        });
    }
}

/// Writes a stub `nym-proxy` into `dir` that answers `--discover` with one
/// Exit Node and otherwise announces `socks5_addr` and its exit with the
/// minted stdout tokens before sleeping forever, so the supervisor reads
/// readiness and the birth proves through the test-hosted endpoint.
#[cfg(feature = "nym")]
fn write_stub_proxy(
    dir: &std::path::Path,
    socks5_addr: std::net::SocketAddr,
) -> std::path::PathBuf {
    let path = dir.join("stub-nym-proxy");
    let exit_prefix = zingo_netutils::NYM_EXIT_LINE_PREFIX;
    let socks5_prefix = zingo_netutils::SOCKS5_ADDR_LINE_PREFIX;
    // The advertised directory, built here so its shell indentation is not
    // subject to the string continuations below.
    let advertised: String = (1..=STUB_EXIT_COUNT)
        .map(|nth| format!("    echo \"{exit_prefix}{STUB_EXIT_NODE}-{nth}\"\n"))
        .collect();
    let script = format!(
        "#!/bin/sh\n\
         pinned=\n\
         wants_exit=0\n\
         for arg in \"$@\"; do\n\
         \x20 if [ \"$arg\" = --discover ]; then\n\
{advertised}\
         \x20   exit 0\n\
         \x20 fi\n\
         \x20 if [ \"$wants_exit\" = 1 ]; then\n\
         \x20   pinned=\"$arg\"\n\
         \x20   wants_exit=0\n\
         \x20 fi\n\
         \x20 if [ \"$arg\" = --exit ]; then\n\
         \x20   wants_exit=1\n\
         \x20 fi\n\
         done\n\
         [ -n \"$pinned\" ] || pinned={STUB_EXIT_NODE}-1\n\
         echo \"{socks5_prefix}{socks5_addr}\"\n\
         echo \"{exit_prefix}$pinned\"\n\
         exec sleep infinity\n",
    );
    std::fs::write(&path, script).expect("write the stub proxy");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("make the stub proxy executable");
    }
    path
}

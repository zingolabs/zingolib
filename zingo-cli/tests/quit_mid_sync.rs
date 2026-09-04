//! The acceptance test for ZIN-70: a `quit` typed while the scan is running
//! must end the session within a bound and leave a loadable wallet, instead
//! of waiting out the sync behind the wallet lock.

#![forbid(unsafe_code)]

#[path = "support/cli_session.rs"]
mod cli_session;

use std::io::Write as _;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The log filter the session runs under, opening pepper-sync's debug stream so the scan-progress marker reaches the log.
const LOG_FILTER: &str = "info,pepper_sync=debug";

/// The pepper-sync debug line following a scanned batch's processing under the wallet lock, which proves the scan holds the lock quit must contend with.
const SCAN_PROGRESS_MARKER: &str = "Scan results processed.";

/// The ceiling on session startup plus the first scanned batch reaching the log.
const SCAN_EVIDENCE_DEADLINE: Duration = Duration::from_secs(300);

/// The ceiling between typing `quit` and process exit, far below the minutes the remaining sync would take, so a shutdown that waits out the scan fails the test.
const QUIT_EXIT_BOUND: Duration = Duration::from_secs(120);

/// The acceptance test for a quit issued mid-sync: the session must exit within `QUIT_EXIT_BOUND` and leave a wallet file a fresh session loads.
#[test]
#[ignore = "network-bound acceptance test; run explicitly"]
fn quit_mid_sync_exits_within_bound_and_leaves_a_loadable_wallet() {
    let indexer = std::env::var("QUIT_MID_SYNC_INDEXER")
        .unwrap_or_else(|_| cli_session::DEFAULT_INDEXER.to_string());
    let cli = env!("CARGO_BIN_EXE_zingo-cli");
    let proxy = cli_session::nym_proxy_beside(cli);
    let data_dir = tempfile::tempdir().expect("a wallet tempdir opens");
    let log_path = data_dir.path().join("cli.log");

    let mut child = Command::new(cli)
        .env("RUST_LOG", LOG_FILTER)
        .env("ZINGO_NYM_PROXY", &proxy)
        .arg("--data-dir")
        .arg(data_dir.path())
        .arg("--log-file")
        .arg(&log_path)
        .arg("--server")
        .arg(&indexer)
        .arg("--seed")
        .arg(cli_session::MNEMONIC)
        .arg("--birthday")
        .arg(cli_session::BIRTHDAY.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("zingo-cli spawns");

    // Hold quit until a scanned batch's results have been processed: the
    // span-open marker precedes any scan work, so a quit gated on it can
    // land while the wallet lock is still free and prove nothing.
    let scan_evidence_deadline = Instant::now() + SCAN_EVIDENCE_DEADLINE;
    loop {
        if std::fs::read_to_string(&log_path)
            .unwrap_or_default()
            .contains(SCAN_PROGRESS_MARKER)
        {
            break;
        }
        if let Some(status) = child.try_wait().expect("the child polls") {
            panic!("zingo-cli exited {status} before the scan processed a batch");
        }
        assert!(
            Instant::now() < scan_evidence_deadline,
            "no {SCAN_PROGRESS_MARKER:?} line reached the log file within \
             {SCAN_EVIDENCE_DEADLINE:?}"
        );
        std::thread::sleep(cli_session::CHILD_POLL);
    }

    child
        .stdin
        .as_mut()
        .expect("stdin was piped")
        .write_all(b"quit\n")
        .expect("quit reaches the session");
    let quit_sent = Instant::now();

    loop {
        if let Some(status) = child.try_wait().expect("the child polls") {
            assert!(status.success(), "zingo-cli exited {status} after quit");
            break;
        }
        if quit_sent.elapsed() > QUIT_EXIT_BOUND {
            child.kill().expect("the child dies");
            child.wait().expect("the killed child reaps");
            panic!(
                "quit did not end the session within {QUIT_EXIT_BOUND:?}; \
                 the save-task shutdown is again waiting behind the scan"
            );
        }
        std::thread::sleep(cli_session::CHILD_POLL);
    }

    // A second session over the same data dir proves the interrupted
    // shutdown persisted a loadable wallet: with no consent act the
    // launch is offline, and the one-shot command must answer from the
    // wallet file alone.
    let reload = Command::new(cli)
        .arg("--data-dir")
        .arg(data_dir.path())
        .arg("--nosync")
        .arg("addresses")
        .output()
        .expect("the reload session runs");
    assert!(
        reload.status.success(),
        "the wallet saved by the interrupted quit did not load: {}\nstdout:\n{}\nstderr:\n{}",
        reload.status,
        String::from_utf8_lossy(&reload.stdout),
        String::from_utf8_lossy(&reload.stderr),
    );
}

#![forbid(unsafe_code)]

use std::io::Write as _;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use zingolib::lightclient::sync;

/// The number of mainnet blocks below the authoring-day tip the wallet's birthday sits, deep enough that the initial sync far outlasts the quit bound.
const SYNC_WINDOW: u32 = 20_000;

/// The mainnet chain height on the day the test was authored.
const TIP_AT_AUTHORING: u32 = 3_445_000;

/// The fixed wallet birthday, one sync window below the authoring-day tip.
const BIRTHDAY: u32 = TIP_AT_AUTHORING - SYNC_WINDOW;

/// The indexer URI the session syncs against, overridable via `QUIT_MID_SYNC_INDEXER`.
const DEFAULT_INDEXER: &str = "https://zec.rocks:443";

/// The ceiling on session startup plus the sync span opening in the log.
const SYNC_OPEN_DEADLINE: Duration = Duration::from_secs(300);

/// The ceiling between typing `quit` and process exit, far below the minutes the remaining sync would take, so a shutdown that waits out the scan fails the test.
const QUIT_EXIT_BOUND: Duration = Duration::from_secs(120);

/// The cadence at which the harness polls the log file and the child for exit.
const POLL: Duration = Duration::from_millis(500);

/// A BIP-39 mnemonic holding no funds, so the interrupted sync scans pure chain data.
const MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon art";

/// The acceptance test for a quit issued mid-sync: the session must exit within `QUIT_EXIT_BOUND` and leave a wallet file a fresh session loads.
#[test]
#[ignore = "network-bound acceptance test; run explicitly"]
fn quit_mid_sync_exits_within_bound_and_leaves_a_loadable_wallet() {
    let indexer =
        std::env::var("QUIT_MID_SYNC_INDEXER").unwrap_or_else(|_| DEFAULT_INDEXER.to_string());
    let cli = env!("CARGO_BIN_EXE_zingo-cli");
    // The nym-proxy built from the same checkout sits beside the CLI binary,
    // so a session whose startup provisions the mixnet finds a
    // protocol-matched proxy.
    let proxy = std::path::Path::new(cli).with_file_name("nym-proxy");
    let data_dir = tempfile::tempdir().expect("a wallet tempdir opens");
    let log_path = data_dir.path().join("cli.log");

    let mut child = Command::new(cli)
        .env("RUST_LOG", "info")
        .env("ZINGO_NYM_PROXY", &proxy)
        .arg("--data-dir")
        .arg(data_dir.path())
        .arg("--log-file")
        .arg(&log_path)
        .arg("--server")
        .arg(&indexer)
        .arg("--seed")
        .arg(MNEMONIC)
        .arg("--birthday")
        .arg(BIRTHDAY.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("zingo-cli spawns");

    // Hold quit until the engine's own span marker proves a sync is in
    // flight; quitting any earlier would exercise the idle path instead.
    let sync_open_deadline = Instant::now() + SYNC_OPEN_DEADLINE;
    loop {
        if std::fs::read_to_string(&log_path)
            .unwrap_or_default()
            .contains(sync::SYNC_SPAN_OPEN)
        {
            break;
        }
        if let Some(status) = child.try_wait().expect("the child polls") {
            panic!("zingo-cli exited {status} before the sync span opened");
        }
        assert!(
            Instant::now() < sync_open_deadline,
            "no {} marker reached the log file within {SYNC_OPEN_DEADLINE:?}",
            sync::SYNC_SPAN_OPEN
        );
        std::thread::sleep(POLL);
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
        std::thread::sleep(POLL);
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

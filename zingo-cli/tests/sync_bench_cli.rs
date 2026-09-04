//! A timed one-shot zingo-cli sync over the same fixed mainnet window as
//! zingolib's `sync_bench`.
//!
//! The test drives the real `zingo-cli` binary: a fresh unfunded wallet
//! restored from seed at the fixed birthday, `--server` as the launch
//! consent act, `--waitsync` to hold the session until sync completes, and
//! the one-shot `height` command whose output proves the wallet reached the
//! chain tip. The measured time is the whole user experience — session
//! startup, any transport bootstrap the commit performs, and the sync — so
//! run at two commits it bisects regressions the library-level benchmark
//! cannot see.

#![forbid(unsafe_code)]

#[path = "support/cli_session.rs"]
mod cli_session;

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The default seconds of budget, overridable via `SYNC_BENCH_BUDGET_SECS`.
const DEFAULT_BUDGET_SECS: u64 = 540;

#[test]
#[ignore = "network-bound timing benchmark; run explicitly"]
fn cli_syncs_20k_mainnet_blocks_within_budget() {
    let budget = Duration::from_secs(
        std::env::var("SYNC_BENCH_BUDGET_SECS")
            .ok()
            .and_then(|raw| raw.parse().ok())
            .unwrap_or(DEFAULT_BUDGET_SECS),
    );
    // An empty SYNC_BENCH_INDEXER omits --server entirely, measuring the
    // commit's own indexer resolution instead of a pin.
    let indexer = std::env::var("SYNC_BENCH_INDEXER")
        .unwrap_or_else(|_| cli_session::DEFAULT_INDEXER.to_string());
    // Whitespace-split launch flags for A/B cells the fixed grammar lacks,
    // e.g. `--no-mixnet` at commits that still offer it, or `--online`.
    let extra_args = std::env::var("SYNC_BENCH_EXTRA_ARGS").unwrap_or_default();

    let cli = env!("CARGO_BIN_EXE_zingo-cli");
    let proxy = cli_session::nym_proxy_beside(cli);
    let data_dir = tempfile::tempdir().expect("a wallet tempdir opens");

    let mut args: Vec<String> = vec![
        "--data-dir".into(),
        data_dir
            .path()
            .to_str()
            .expect("the tempdir path renders")
            .into(),
        "--seed".into(),
        cli_session::MNEMONIC.into(),
        "--birthday".into(),
        cli_session::BIRTHDAY.to_string(),
        "--waitsync".into(),
    ];
    if !indexer.is_empty() {
        args.push("--server".into());
        args.push(indexer.clone());
    }
    args.extend(extra_args.split_whitespace().map(String::from));
    args.push("height".into());

    let mut child = Command::new(cli)
        .args(&args)
        .env("ZINGO_NYM_PROXY", &proxy)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("zingo-cli spawns");
    let started = Instant::now();

    let status = loop {
        if let Some(status) = child.try_wait().expect("the child polls") {
            break status;
        }
        if started.elapsed() > budget {
            child.kill().expect("the child dies");
            child.wait().expect("the killed child reaps");
            panic!(
                "SYNC_BENCH_CLI: budget of {}s exceeded at {} blocks from {}",
                budget.as_secs(),
                cli_session::SYNC_WINDOW,
                cli_session::BIRTHDAY
            );
        }
        std::thread::sleep(cli_session::CHILD_POLL);
    };
    let elapsed = started.elapsed().as_secs_f64();

    let mut stdout = String::new();
    std::io::Read::read_to_string(
        child.stdout.as_mut().expect("stdout was piped"),
        &mut stdout,
    )
    .expect("the child's stdout reads");
    assert!(
        status.success(),
        "SYNC_BENCH_CLI: zingo-cli exited {status} after {elapsed:.1}s; stdout:\n{stdout}"
    );

    // The one-shot `height` output proves the sync reached the chain tip;
    // without this, a commit that silently skips sync would time as fast.
    let synced_height: u32 = stdout
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|digits| digits.parse().ok())
        .max()
        .expect("the height output carries a number");
    assert!(
        synced_height >= cli_session::TIP_AT_AUTHORING,
        "SYNC_BENCH_CLI: wallet height {synced_height} never reached {}; \
         the session did not sync. stdout:\n{stdout}",
        cli_session::TIP_AT_AUTHORING
    );

    println!(
        "SYNC_BENCH_CLI: {} blocks from {} via {indexer} to height \
         {synced_height} in {elapsed:.1}s",
        cli_session::SYNC_WINDOW,
        cli_session::BIRTHDAY
    );
}

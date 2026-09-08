//! An offline acceptance test for the interactive session's exit arms.
//!
//! ZIN-70's fix taught the typed `quit` command to stop sync and drain the
//! save task, but a session also ends through rustyline's Ctrl-C, Ctrl-D,
//! and terminal-error arms. This test ends an offline session by closing
//! stdin — the Ctrl-D arm — and asserts the same teardown ran, using the
//! quit command's trailer as the evidence. All three arms share the one
//! dispatch, so the reachable arm stands in for the pair a piped harness
//! cannot trigger.

#![forbid(unsafe_code)]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The ceiling on the whole offline session, from spawn to exit.
const SESSION_BOUND: Duration = Duration::from_secs(60);

/// The cadence at which the harness polls the child for exit.
const CHILD_POLL: Duration = Duration::from_millis(100);

/// The trailer the quit command prints, proving the teardown ran.
const QUIT_TRAILER: &str = "Zingo CLI quit successfully.";

/// The acceptance test for an EOF-ended session: the exit arm must dispatch the quit teardown before the process exits.
#[test]
fn an_eof_ended_session_still_runs_the_quit_teardown() {
    let cli = env!("CARGO_BIN_EXE_zingo-cli");
    let data_dir = tempfile::tempdir().expect("a wallet tempdir opens");

    // A fresh wallet under --offline is the deliberate no-network launch,
    // so the session boots without any indexer or seed restore.
    let mut child = Command::new(cli)
        .arg("--data-dir")
        .arg(data_dir.path())
        .arg("--offline")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("zingo-cli spawns");

    // Closing stdin before any command is the piped spelling of Ctrl-D:
    // rustyline reports Eof on the first read.
    drop(child.stdin.take());

    let deadline = Instant::now() + SESSION_BOUND;
    loop {
        if child.try_wait().expect("the child polls").is_some() {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().expect("the child dies");
            child.wait().expect("the killed child reaps");
            panic!("the session did not exit within {SESSION_BOUND:?} after EOF");
        }
        std::thread::sleep(CHILD_POLL);
    }

    let output = child.wait_with_output().expect("the exited child reaps");
    let transcript = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        output.status.success(),
        "the EOF-ended session exited {}: {transcript}",
        output.status
    );
    assert!(
        transcript.contains("CTRL-D"),
        "the session did not end through the Eof arm: {transcript}"
    );
    assert!(
        transcript.contains(QUIT_TRAILER),
        "the Eof arm exited without running the quit teardown: {transcript}"
    );
}

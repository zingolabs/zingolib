//! Times sync inside a real `run-cli --online` session, mixnet boot included.
//!
//! An in-process harness measures the sync engine alone, which answers
//! nothing about a session where four proven exits bootstrap beside the
//! scan. This drives the CLI the way a user does: `makers run-cli --online`
//! spawns the proxy, proves the quartet, runs the Server-Selection Sweep,
//! and only then launches sync.
//!
//! The headline number comes from the sync engine's own clock. The task
//! that runs the scan opens and closes a span around it and logs the
//! elapsed milliseconds, so this reads one measurement taken inside the
//! task that did the work. Nothing here polls a status or watches a prompt
//! redraw, and no cross-process timestamp arithmetic enters the number.
#![forbid(unsafe_code)]

use std::io::Write as _;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use workbench::{repo_root, run};

/// Agreeing representations of `zingolib::lightclient::sync`'s minted log
/// markers, restated because the workbench is deliberately std-only and
/// takes no production dependency; these must not drift from that crate.
const SYNC_SPAN_OPEN: &str = "SYNC_SPAN=open";

/// The marker closing the span, followed by the engine's own duration.
const SYNC_SPAN_CLOSE: &str = "SYNC_SPAN=close";

/// The unit suffix the closing marker's duration carries.
const MILLIS_SUFFIX: &str = "ms";

/// The log filter the session runs under, since an empty one silences the
/// markers this reads.
const LOG_FILTER: &str = "info";

/// The indexer every run pins, so arms compare like with like.
const PINNED_INDEXER: &str = "https://zec.rocks:443";

/// A BIP-39 mnemonic holding no funds, so the scan measures pure scanning.
const GUARD_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon art";

/// Sessions one arm runs when `--runs` names no other count.
const DEFAULT_RUNS: usize = 3;

/// How long one session may take before it counts as failed.
const RUN_BUDGET: Duration = Duration::from_secs(900);

/// How often the driver rereads the log for the markers.
const LOG_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The degree of freedom a sample standard deviation gives up to its mean.
const BESSEL_CORRECTION: usize = 1;

fn main() {
    run("sync-bench", bench, |()| {})
}

/// One session's outcome.
enum Outcome {
    /// Boot measured by wall clock, sync by the engine's own.
    Synced {
        boot: Duration,
        sync: Duration,
    },
    Failed {
        detail: String,
    },
}

/// What the invocation asked for.
struct Request {
    birthday: u32,
    label: String,
    runs: usize,
    /// Whether the session builds without the mixnet.
    clearnet: bool,
}

fn parse_request() -> Result<Request, Vec<String>> {
    let mut args = std::env::args().skip(1);
    let mut birthday = None;
    let mut label = String::from("unlabelled");
    let mut runs = DEFAULT_RUNS;
    let mut clearnet = false;
    while let Some(argument) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .ok_or_else(|| vec![format!("{name} needs a value")])
        };
        match argument.as_str() {
            "--birthday" => {
                let raw = value("--birthday")?;
                birthday = Some(
                    raw.parse()
                        .map_err(|e| vec![format!("--birthday {raw}: {e}")])?,
                );
            }
            // The mixnet is a default capability (ADR 0026), so an Online
            // session boots a proxy, proves a quartet, and sweeps before the
            // scan. `--clearnet` builds without it, which is what makes a
            // pair of runs attribute the mixnet's cost to the mixnet rather
            // than leaving it inside one number.
            "--clearnet" => clearnet = true,
            "--label" => label = value("--label")?,
            "--runs" => {
                let raw = value("--runs")?;
                runs = raw
                    .parse()
                    .map_err(|e| vec![format!("--runs {raw}: {e}")])?;
            }
            other => return Err(vec![format!("unknown argument: {other}")]),
        }
    }
    let birthday = birthday.ok_or_else(|| {
        vec![
            "--birthday <height> is required, and both arms of an A/B must use \
             the same one so they scan the same window"
                .to_string(),
        ]
    })?;
    Ok(Request {
        birthday,
        label,
        runs,
        clearnet,
    })
}

fn bench() -> Result<(), Vec<String>> {
    let request = parse_request()?;
    let root = repo_root()?;

    let mut syncs: Vec<Duration> = Vec::new();
    let mut boots: Vec<Duration> = Vec::new();
    for index in 0..request.runs {
        match session(&root, request.birthday, request.clearnet)? {
            Outcome::Synced { boot, sync } => {
                eprintln!(
                    "sync-bench: {:2}/{} boot {:.1}s, sync {:.3}s",
                    index + 1,
                    request.runs,
                    boot.as_secs_f64(),
                    sync.as_secs_f64()
                );
                boots.push(boot);
                syncs.push(sync);
            }
            Outcome::Failed { detail } => eprintln!(
                "sync-bench: {:2}/{} failed: {detail}",
                index + 1,
                request.runs
            ),
        }
    }
    report(&request.label, &boots, &syncs);
    Ok(())
}

/// Drives one session, reading both markers out of its log.
fn session(root: &Path, birthday: u32, clearnet: bool) -> Result<Outcome, Vec<String>> {
    let scratch = root.join("target").join("sync-bench");
    // A fresh wallet each run, so no session resumes a partial scan.
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch)
        .map_err(|e| vec![format!("cannot make {}: {e}", scratch.display())])?;
    let log_path = scratch.join("session.log");

    let mut command = Command::new("makers");
    command
        .current_dir(root)
        .env("RUST_LOG", LOG_FILTER)
        .arg("run-cli");
    if clearnet {
        command.arg("--clearnet");
    }
    command
        .arg("--online")
        .arg("--server")
        .arg(PINNED_INDEXER)
        .arg("--seed")
        .arg(GUARD_MNEMONIC)
        .arg("--birthday")
        .arg(birthday.to_string())
        .arg("--data-dir")
        .arg(scratch.join("wallets"))
        .arg("--log-file")
        .arg(&log_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let mut child = command
        .spawn()
        .map_err(|e| vec![format!("cannot spawn makers run-cli: {e}")])?;

    let spawned = Instant::now();
    let mut launched: Option<Instant> = None;
    let outcome = loop {
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        if launched.is_none() && log.contains(SYNC_SPAN_OPEN) {
            launched = Some(Instant::now());
        }
        if let Some(millis) = closing_millis(&log) {
            break match launched {
                Some(at) => Outcome::Synced {
                    boot: at.duration_since(spawned),
                    sync: Duration::from_millis(millis),
                },
                None => Outcome::Failed {
                    detail: "the span closed without opening".to_string(),
                },
            };
        }
        if let Ok(Some(status)) = child.try_wait() {
            break Outcome::Failed {
                detail: format!("the session exited early ({status})"),
            };
        }
        if spawned.elapsed() >= RUN_BUDGET {
            break Outcome::Failed {
                detail: format!("budget of {}s exceeded", RUN_BUDGET.as_secs()),
            };
        }
        std::thread::sleep(LOG_POLL_INTERVAL);
    };

    quit(&mut child);
    Ok(outcome)
}

/// The engine's own duration, from the line closing the span.
fn closing_millis(log: &str) -> Option<u64> {
    let tail = log.split(SYNC_SPAN_CLOSE).nth(1)?;
    let digits: String = tail
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    // A truncated final line can hold the marker before its whole duration,
    // so a reading is taken only once the unit follows the digits.
    tail.trim_start()
        .get(digits.len()..)?
        .starts_with(MILLIS_SUFFIX)
        .then(|| digits.parse().ok())
        .flatten()
}

/// Ends the session the way a user does, then reaps it.
fn quit(child: &mut Child) {
    if let Some(stdin) = child.stdin.as_mut() {
        let _ = writeln!(stdin, "quit");
        let _ = stdin.flush();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Prints both measurements against the label the arm carries.
fn report(label: &str, boots: &[Duration], syncs: &[Duration]) {
    println!("\nSYNC_BENCH_TAG: {label}");
    println!("sessions: {}", syncs.len());
    distribution("spawn to sync launch (wall clock)", boots);
    distribution("sync span (engine clock)", syncs);
}

/// Prints one measurement's spread and its raw samples.
fn distribution(what: &str, samples: &[Duration]) {
    if samples.is_empty() {
        return;
    }
    let millis: Vec<u128> = samples.iter().map(Duration::as_millis).collect();
    let mean = millis.iter().map(|&each| each as f64).sum::<f64>() / millis.len() as f64;
    println!("{what}:");
    println!("  mean   {:.3}s", mean / 1_000.0);
    if let Some(freedom) = millis
        .len()
        .checked_sub(BESSEL_CORRECTION)
        .filter(|&freedom| freedom > 0)
    {
        let squares: f64 = millis
            .iter()
            .map(|&each| (each as f64 - mean).powi(2))
            .sum();
        println!(
            "  stdev  {:.3}s",
            (squares / freedom as f64).sqrt() / 1_000.0
        );
    }
    println!(
        "  samples {:?}",
        millis
            .iter()
            .map(|&ms| format!("{:.3}s", ms as f64 / 1_000.0))
            .collect::<Vec<_>>()
    );
}

//! Measures how often a drawn Exit Node is unavailable, and how long a
//! birth waits to find out.
//!
//! ADR 0043 put a quarter to a third of Nym Exit Nodes at carrying
//! nothing, and ADR 0045 leans on that rate: it decides how many lanes a
//! boot needs to fill its quartet. The rate has never been measured from
//! this workspace, and neither has the announcement latency the
//! `EXIT_ANNOUNCEMENT_GRACE` budget was chosen without.
//!
//! One trial spawns `nym-proxy` against one exit drawn from the directory,
//! waits for it to announce that exit, and stops it. An announcement is
//! the exit reaching readiness; silence to the grace is an exit that could
//! not be reached at all. This measures reachability, not the Sentinel's
//! carries-nothing verdict, which needs a round trip through the tunnel.
#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use workbench::{repo_root, run};

/// Agreeing representations of `zingo_netutils`'s minted stdout tokens,
/// restated because the workbench is deliberately std-only and takes no
/// production dependency; these must not drift from that crate.
const NYM_EXIT_LINE_PREFIX: &str = "NYM_EXIT=";

/// How long one birth may take to announce its exit before the trial
/// counts it unreachable. Agrees with
/// `zingo_netutils::time::EXIT_ANNOUNCEMENT_GRACE`.
const ANNOUNCEMENT_GRACE: Duration = Duration::from_secs(25);

/// How many births one trial run makes when `--births` names no other count.
const DEFAULT_TRIALS: usize = 100;

/// How often the reader wakes to check whether the grace has elapsed.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

fn main() {
    run("birth-trial", trial, |()| {})
}

/// One birth's outcome: the exit announced within the grace, the proxy gave
/// up and exited before announcing, or it stayed silent to the grace.
enum Outcome {
    Announced { elapsed: Duration },
    Exited { elapsed: Duration },
    Unreachable,
}

/// The count of births to make, from `--births N` or the default.
fn trials_requested() -> Result<usize, Vec<String>> {
    let mut args = std::env::args().skip(1);
    let mut trials = DEFAULT_TRIALS;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--births" => {
                let count = args
                    .next()
                    .ok_or_else(|| vec!["--births needs a count".to_string()])?;
                trials = count
                    .parse()
                    .map_err(|e| vec![format!("--births {count}: {e}")])?;
            }
            other => return Err(vec![format!("unknown argument: {other}")]),
        }
    }
    Ok(trials)
}

fn trial() -> Result<(), Vec<String>> {
    let trials = trials_requested()?;
    let root = repo_root()?;
    let proxy = root.join("target").join("debug").join("nym-proxy");
    if !proxy.exists() {
        return Err(vec![format!(
            "no nym-proxy at {}: build it with `makers bundle-nym-proxy`",
            proxy.display()
        )]);
    }

    let exits = discover(&proxy)?;
    eprintln!("birth-trial: {} exits advertised", exits.len());
    if exits.len() < trials {
        return Err(vec![format!(
            "only {} exits advertised, fewer than the {trials} trials: \
             a trial would draw one twice",
            exits.len()
        )]);
    }

    let mut announced: Vec<Duration> = Vec::new();
    let mut exited = 0usize;
    let mut unreachable = 0usize;
    for (index, exit) in exits.iter().take(trials).enumerate() {
        match birth(&proxy, exit)? {
            Outcome::Announced { elapsed } => {
                eprintln!(
                    "birth-trial: {:3}/{trials} announced in {}ms",
                    index + 1,
                    elapsed.as_millis()
                );
                announced.push(elapsed);
            }
            Outcome::Exited { elapsed } => {
                eprintln!(
                    "birth-trial: {:3}/{trials} exited after {}ms without announcing",
                    index + 1,
                    elapsed.as_millis()
                );
                exited += 1;
            }
            Outcome::Unreachable => {
                eprintln!("birth-trial: {:3}/{trials} unreachable", index + 1);
                unreachable += 1;
            }
        }
    }
    report(&announced, exited, unreachable);
    Ok(())
}

/// Every Exit Node the directory advertises, in the order discovery gave
/// them, which the proxy already shuffled.
fn discover(proxy: &Path) -> Result<Vec<String>, Vec<String>> {
    let output = Command::new(proxy)
        .arg("--discover")
        .output()
        .map_err(|e| vec![format!("cannot run {}: {e}", proxy.display())])?;
    if !output.status.success() {
        return Err(vec![format!("discovery failed ({})", output.status)]);
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix(NYM_EXIT_LINE_PREFIX))
        .map(str::to_string)
        .collect())
}

/// Spawns one proxy pinned to `exit` and times how long it takes to
/// announce that exit, stopping it either way.
fn birth(proxy: &Path, exit: &str) -> Result<Outcome, Vec<String>> {
    // The proxy races its bootstrap against its own stdin closing, the
    // watchdog that stops an orphan outliving its parent. A trial that lets
    // the child inherit a closed stdin therefore measures the watchdog and
    // reports every exit unreachable, so this pipe stays open — unread and
    // unwritten — for the whole birth. Its stderr is inherited so a refusal
    // reaches the operator rather than the void.
    let mut child = Command::new(proxy)
        .arg("--exit")
        .arg(exit)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| vec![format!("cannot spawn {}: {e}", proxy.display())])?;

    let started = Instant::now();
    let stdout = child.stdout.take().expect("stdout was piped");
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.starts_with(NYM_EXIT_LINE_PREFIX) && sender.send(()).is_err() {
                return;
            }
        }
    });

    let outcome = loop {
        // The announcement is read before the exit status, so a proxy that
        // announces and then stops still counts as having announced.
        if receiver.try_recv().is_ok() {
            break Outcome::Announced {
                elapsed: started.elapsed(),
            };
        }
        if matches!(child.try_wait(), Ok(Some(_))) {
            break Outcome::Exited {
                elapsed: started.elapsed(),
            };
        }
        if started.elapsed() >= ANNOUNCEMENT_GRACE {
            break Outcome::Unreachable;
        }
        std::thread::sleep(POLL_INTERVAL);
    };

    let _ = child.kill();
    let _ = child.wait();
    Ok(outcome)
}

/// Prints the rate and the latency distribution the design was missing.
fn report(announced: &[Duration], exited: usize, unreachable: usize) {
    let total = announced.len() + exited + unreachable;
    let failed = exited + unreachable;
    println!("\nbirths:            {total}");
    println!("announced:         {}", announced.len());
    println!("exited early:      {exited}");
    println!("silent to grace:   {unreachable}");
    if total > 0 {
        println!(
            "failure rate:      {:.1}%",
            100.0 * failed as f64 / total as f64
        );
    }
    if announced.is_empty() {
        return;
    }
    let mut sorted: Vec<u128> = announced.iter().map(Duration::as_millis).collect();
    sorted.sort_unstable();
    let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
    println!(
        "announcement latency of the {} that answered:",
        sorted.len()
    );
    println!("  min    {}ms", sorted[0]);
    println!("  median {}ms", at(0.5));
    println!("  p90    {}ms", at(0.9));
    println!("  max    {}ms", sorted[sorted.len() - 1]);
    println!(
        "  grace  {}ms (the budget these are measured against)",
        ANNOUNCEMENT_GRACE.as_millis()
    );
}

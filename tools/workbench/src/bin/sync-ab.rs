//! Compares two commits' sync rate, interleaved, with everything but the
//! commits held fixed.
//!
//! A benchmark that varies more than one thing measures nothing. This holds
//! the indexer, the wallet seed, the scanned window, the performance level,
//! and the build profile constant, runs the two arms alternately so warm-up
//! and machine load fall on both equally, and reports outputs per second,
//! which is the rate scan cost actually tracks.
//!
//! The window is pinned by a discarded warm-up run: the first run learns the
//! birthday from the live tip and every measured run is then given that same
//! birthday, so both arms scan from the same block without this tool ever
//! querying the chain. The warm-up is discarded on its own merits too, since
//! a first run reliably reads slower than its successors.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use workbench::{repo_root, run};

/// The harness both arms run, copied into each worktree from the invoking
/// checkout so the two arms measure their own code and not their own
/// harnesses.
const HARNESS: &str = "zingolib/examples/sync_timing.rs";

/// The indexer every run pins, so neither arm meets a different server.
const INDEXER: &str = "https://zec.rocks:443";

/// The performance level every run pins.
const LEVEL: &str = "high";

/// Measured runs per arm, after the discarded warm-up.
const RUNS: usize = 3;

/// The marker the harness prints its reading on.
const READING: &str = "SYNC_PERF_TAG:";

/// The degree of freedom a sample standard deviation gives up to its mean.
const BESSEL_CORRECTION: usize = 1;

/// How far the two arms' output counts may differ before the comparison is
/// reported as drifted: the tip advances during a run, so a later run scans
/// a few more outputs than an earlier one.
const OUTPUT_DRIFT_TOLERANCE: f64 = 0.01;

fn main() {
    run("sync-ab", compare, |()| {})
}

/// One arm's build and its readings.
struct Arm {
    commit: String,
    worktree: PathBuf,
    rates: Vec<f64>,
    outputs: Vec<u64>,
}

/// One harness reading.
struct Reading {
    birthday: u32,
    outputs: u64,
    rate: f64,
}

fn compare() -> Result<(), Vec<String>> {
    let commits: Vec<String> = std::env::args().skip(1).collect();
    let [left, right] = commits.as_slice() else {
        return Err(vec![
            "sync-ab takes exactly two commits, the pair to compare; every \
             other parameter is fixed so the comparison means something"
                .to_string(),
        ]);
    };
    let root = repo_root()?;

    let mut arms = [
        prepare(&root, left)?,
        prepare(&root, right)?,
    ];

    // The warm-up establishes the window and is discarded: a first run reads
    // slower than its successors, and both arms deserve the same treatment.
    eprintln!("sync-ab: warming up and pinning the window...");
    let birthday = measure(&arms[0].worktree, None)?.birthday;
    let _ = measure(&arms[1].worktree, Some(birthday))?;
    eprintln!("sync-ab: window pinned at birthday {birthday}");

    for round in 1..=RUNS {
        // The order alternates, so whatever advantage a second run enjoys
        // falls on each arm equally instead of on whichever went last.
        let order: [usize; 2] = if round % 2 == 1 { [0, 1] } else { [1, 0] };
        for index in order {
            let arm = &mut arms[index];
            let reading = measure(&arm.worktree, Some(birthday))?;
            eprintln!(
                "sync-ab: {round}/{RUNS} {} {:.0} outputs/s over {} outputs",
                short(&arm.commit),
                reading.rate,
                reading.outputs
            );
            arm.rates.push(reading.rate);
            arm.outputs.push(reading.outputs);
        }
    }

    report(&arms);
    for arm in &arms {
        retire(&root, &arm.worktree);
    }
    Ok(())
}

/// Builds one arm in its own worktree, carrying this checkout's harness in.
fn prepare(root: &Path, commit: &str) -> Result<Arm, Vec<String>> {
    let worktree = root
        .parent()
        .ok_or_else(|| vec!["the repository has no parent directory".to_string()])?
        .join(format!("sync-ab-{}", slug(commit)));
    // A stale worktree from an interrupted run would refuse the add, and
    // its absence is the ordinary case rather than a failure worth showing.
    let _ = Command::new("git")
        .current_dir(root)
        .args(["worktree", "remove", "--force"])
        .arg(&worktree)
        .stderr(std::process::Stdio::null())
        .status();

    eprintln!("sync-ab: preparing {}", short(commit));
    git(root, &["worktree", "add", "--detach", &worktree.to_string_lossy(), commit])?;

    let harness = worktree.join(HARNESS);
    std::fs::create_dir_all(harness.parent().expect("the harness has a directory"))
        .map_err(|e| vec![format!("cannot make the harness directory: {e}")])?;
    std::fs::copy(root.join(HARNESS), &harness)
        .map_err(|e| vec![format!("cannot carry the harness into {commit}: {e}")])?;

    let built = Command::new("cargo")
        .current_dir(&worktree)
        .args([
            "build",
            "--quiet",
            "--release",
            "-p",
            "zingolib",
            "--example",
            "sync_timing",
        ])
        .status()
        .map_err(|e| vec![format!("cannot build {commit}: {e}")])?;
    if !built.success() {
        return Err(vec![format!("{commit} does not build the harness")]);
    }

    Ok(Arm {
        commit: commit.to_string(),
        worktree,
        rates: Vec::new(),
        outputs: Vec::new(),
    })
}

/// Runs the harness once, pinned to `birthday` when one is given.
fn measure(worktree: &Path, birthday: Option<u32>) -> Result<Reading, Vec<String>> {
    let binary = worktree.join("target/release/examples/sync_timing");
    let mut command = Command::new(&binary);
    command.current_dir(worktree).arg(INDEXER);
    if let Some(birthday) = birthday {
        command.arg(birthday.to_string()).arg(LEVEL);
    }
    let output = command
        .output()
        .map_err(|e| vec![format!("cannot run {}: {e}", binary.display())])?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text
        .lines()
        .find(|line| line.starts_with(READING))
        .ok_or_else(|| {
            vec![format!(
                "no reading from {}: {}",
                binary.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            )]
        })?;
    parse(line)
}

/// Reads the birthday, the output count, and the rate out of one reading.
fn parse(line: &str) -> Result<Reading, Vec<String>> {
    let after = |marker: &str| -> Option<String> {
        let tail = line.split(marker).nth(1)?;
        Some(
            tail.trim_start()
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect(),
        )
    };
    let refusal = |what: &str| vec![format!("no {what} in the reading: {line}")];
    Ok(Reading {
        birthday: after("blocks from")
            .ok_or_else(|| refusal("birthday"))?
            .parse()
            .map_err(|e| vec![format!("unreadable birthday: {e}")])?,
        outputs: after(", ")
            .ok_or_else(|| refusal("output count"))?
            .parse()
            .map_err(|e| vec![format!("unreadable output count: {e}")])?,
        rate: after("= ")
            .ok_or_else(|| refusal("rate"))?
            .parse()
            .map_err(|e| vec![format!("unreadable rate: {e}")])?,
    })
}

/// Prints both arms, their spread, and whether the window held.
fn report(arms: &[Arm; 2]) {
    println!("\nSYNC_AB_TAG: {} vs {}", arms[0].commit, arms[1].commit);
    for arm in arms {
        let mean = mean(&arm.rates);
        print!("{:>12}  mean {mean:>7.0} outputs/s", short(&arm.commit));
        if let Some(spread) = deviation(&arm.rates) {
            print!("  stdev {spread:>6.0}");
        }
        println!(
            "  samples {:?}",
            arm.rates
                .iter()
                .map(|rate| format!("{rate:.0}"))
                .collect::<Vec<_>>()
        );
    }

    let counted: Vec<u64> = arms.iter().flat_map(|arm| arm.outputs.clone()).collect();
    let (low, high) = (
        counted.iter().copied().min().unwrap_or_default(),
        counted.iter().copied().max().unwrap_or_default(),
    );
    let drift = if low == 0 {
        0.0
    } else {
        (high - low) as f64 / low as f64
    };
    println!(
        "outputs scanned: {low} to {high} ({:.2}% drift, the tip advancing mid-run)",
        drift * 100.0
    );
    if drift > OUTPUT_DRIFT_TOLERANCE {
        println!(
            "  WARNING: the arms scanned materially different work, so the \
             rates are not comparable"
        );
    }

    let (left, right) = (mean(&arms[0].rates), mean(&arms[1].rates));
    let gap = if left == 0.0 {
        0.0
    } else {
        (right - left) / left * 100.0
    };
    let noise = deviation(&arms[0].rates)
        .into_iter()
        .chain(deviation(&arms[1].rates))
        .fold(0.0, f64::max);
    println!(
        "difference: {gap:+.1}% ({} against {})",
        short(&arms[1].commit),
        short(&arms[0].commit)
    );
    if (right - left).abs() < noise {
        println!("  within the noisier arm's own deviation, so indistinguishable");
    }
}

/// The arithmetic mean of the sampled rates.
fn mean(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().sum::<f64>() / samples.len() as f64
}

/// The sample standard deviation, which one sample leaves undefined.
fn deviation(samples: &[f64]) -> Option<f64> {
    let freedom = samples.len().checked_sub(BESSEL_CORRECTION)?;
    if freedom == 0 {
        return None;
    }
    let centre = mean(samples);
    let squares: f64 = samples.iter().map(|s| (s - centre).powi(2)).sum();
    Some((squares / freedom as f64).sqrt())
}

/// A commit-ish rendered as one path component, since a branch name may
/// carry a slash and a directory name may not.
fn slug(commit: &str) -> String {
    short(commit)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// A commit short enough to label a row with.
fn short(commit: &str) -> String {
    commit.chars().take(12).collect()
}

/// Removes an arm's worktree, so a comparison leaves no checkout behind.
fn retire(root: &Path, worktree: &Path) {
    let _ = Command::new("git")
        .current_dir(root)
        .args(["worktree", "remove", "--force"])
        .arg(worktree)
        .status();
}

/// Runs one git command, refusing with its stderr.
fn git(root: &Path, args: &[&str]) -> Result<(), Vec<String>> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|e| vec![format!("cannot run git {}: {e}", args.join(" "))])?;
    if output.status.success() {
        return Ok(());
    }
    Err(vec![format!(
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    )])
}

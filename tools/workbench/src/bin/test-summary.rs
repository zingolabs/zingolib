//! `test-summary` — run the three test phases and print a combined summary.
//!
//! Invoked from `makers test all` as
//! `cargo run --bin test-summary -- <nextest args>`. Runs the `packages`,
//! `zingo-cli`, and `libtonode` phases (each in its own CI container via the
//! `makers test` front door), streams each run's output while capturing it,
//! parses the nextest summary line, and aggregates the totals.
//!
//! Phases gate: a failing phase stops the later, more expensive phases from
//! launching (a broken unit test should never cost a libtonode run). The
//! summary table still prints for every phase, marking the ones a failure
//! prevented from running.
//!
//! Adapted from zaino's `tools/test-runner` `live-summary` binary, which
//! instead runs all partitions unconditionally; the gating is deliberate
//! divergence.

#![forbid(unsafe_code)]

use std::error::Error;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

/// The phases `makers test all` runs, in order: the hermetic package tests
/// first, then the live suites from fastest to slowest.
const PHASES: &[&str] = &["packages", "zingo-cli", "libtonode"];

/// One nextest run's tallies, zero where the summary line was absent.
#[derive(Default)]
struct Summary {
    run: u64,
    passed: u64,
    failed: u64,
    skipped: u64,
}

impl Summary {
    fn add(&self, other: &Summary) -> Summary {
        Summary {
            run: self.run + other.run,
            passed: self.passed + other.passed,
            failed: self.failed + other.failed,
            skipped: self.skipped + other.skipped,
        }
    }
}

/// Run one phase through the `makers test` front door, streaming its combined
/// output to our stdout while capturing it for parsing. Returns
/// (exit_code, captured_output).
fn run_phase(phase: &str, forwarded_args: &[String]) -> Result<(i32, String), Box<dyn Error>> {
    // `bash -c '... 2>&1'` merges stderr into stdout so the single captured
    // stream carries the nextest summary line wherever nextest emits it.
    let mut shell_command = format!("makers test {phase}");
    for arg in forwarded_args {
        shell_command.push(' ');
        // Forwarded nextest args are simple flags and filter expressions;
        // single-quote them so filter syntax survives the shell.
        shell_command.push_str(&format!("'{}'", arg.replace('\'', r"'\''")));
    }
    shell_command.push_str(" 2>&1");

    let mut child = Command::new("bash")
        .arg("-c")
        .arg(shell_command)
        // `all` includes the slow:: tests: clear the default nextest filter.
        // The Makefile [env] entry is conditional, so the empty value
        // survives into the container-test child process.
        .env("ZINGOLIB_NEXTEST_FILTER", "")
        .stdout(Stdio::piped())
        .spawn()?;

    let stdout = child
        .stdout
        .take()
        .expect("child stdout is piped: Stdio::piped() was set above");
    let mut captured = String::new();
    for line in BufReader::new(stdout).lines() {
        let line = line?;
        println!("{line}");
        captured.push_str(&line);
        captured.push('\n');
    }

    let code = child.wait()?.code().unwrap_or(1);
    Ok((code, captured))
}

/// Remove ANSI CSI escape sequences (`ESC [ … <final byte>`) so the digits in
/// a summary line aren't split by colour codes.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                // Consume up to and including the final byte (0x40..=0x7E).
                while let Some(&n) = chars.peek() {
                    chars.next();
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            }
            // A lone ESC with no '[' is just dropped.
        } else {
            out.push(c);
        }
    }
    out
}

/// The integer immediately preceding `marker` (after optional spaces), or 0
/// if `marker` is absent. e.g. `count_before("... 8 passed", "passed") == 8`.
fn count_before(line: &str, marker: &str) -> u64 {
    let Some(idx) = line.find(marker) else {
        return 0;
    };
    let head = line[..idx].trim_end();
    let digit_count = head
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .count();
    head[head.len() - digit_count..].parse().unwrap_or(0)
}

/// Parse the last nextest summary line out of a captured run.
///
/// nextest prints e.g.:
///   Summary [ 73.207s] 8 tests run: 8 passed (2 slow), 2 skipped
///   Summary [510.718s] 29 tests run: 23 passed (14 slow), 6 failed, 2 skipped
///   Summary [  1.795s] 1 test run: 0 passed, 1 failed, 114 skipped
fn parse_summary(log: &str) -> Summary {
    // Strip ANSI, then take the last "N test(s) run:" line nextest emitted.
    let line = log
        .lines()
        .map(strip_ansi)
        .filter(|l| l.contains("run:") && l.contains("test"))
        .next_back()
        .unwrap_or_default();

    Summary {
        // The run count is the integer before the word "test" ("N tests run:").
        run: count_before(&line, "test"),
        passed: count_before(&line, "passed"),
        failed: count_before(&line, "failed"),
        skipped: count_before(&line, "skipped"),
    }
}

fn print_row(label: &str, s: &Summary) {
    println!(
        "  {label:<12} {:>4} run, {:>4} passed, {:>4} failed, {:>4} skipped",
        s.run, s.passed, s.failed, s.skipped
    );
}

fn main() -> Result<(), Box<dyn Error>> {
    let forwarded_args: Vec<String> = std::env::args().skip(1).collect();

    let mut results = Vec::new();
    let mut failed = false;
    for phase in PHASES {
        if failed {
            results.push((*phase, None));
            continue;
        }
        println!(">>> test all: running the {phase} phase");
        let (exit_code, log) = run_phase(phase, &forwarded_args)?;
        if exit_code != 0 {
            failed = true;
        }
        results.push((*phase, Some((exit_code, parse_summary(&log)))));
    }

    println!();
    println!("====================== test summary ==========================");
    let mut total = Summary::default();
    for (phase, outcome) in &results {
        match outcome {
            Some((_, summary)) => {
                print_row(&format!("{phase}:"), summary);
                total = total.add(summary);
            }
            None => println!(
                "  {:<12} not run (an earlier phase failed)",
                format!("{phase}:")
            ),
        }
    }
    print_row("TOTAL:", &total);
    println!("==============================================================");

    // A phase that errored without producing a summary line likely failed to
    // build; call it out so the zeros above aren't read as "all clear".
    for (phase, outcome) in &results {
        if let Some((exit_code, summary)) = outcome {
            if *exit_code != 0 && summary.run == 0 {
                println!(
                    "  warning: {phase} produced no nextest summary (build failure?) — see output above."
                );
            }
        }
    }

    if failed {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod parse_summary {
    use super::*;

    fn check(line: &str, run: u64, passed: u64, failed: u64, skipped: u64) {
        let s = parse_summary(line);
        assert_eq!(
            (s.run, s.passed, s.failed, s.skipped),
            (run, passed, failed, skipped)
        );
    }

    #[test]
    fn plural_no_failures() {
        check(
            "Summary [ 73.207s] 8 tests run: 8 passed (2 slow), 2 skipped",
            8,
            8,
            0,
            2,
        );
    }

    #[test]
    fn plural_with_failures() {
        check(
            "Summary [510.718s] 29 tests run: 23 passed (14 slow), 6 failed, 2 skipped",
            29,
            23,
            6,
            2,
        );
    }

    #[test]
    fn singular() {
        check(
            "Summary [  1.795s] 1 test run: 0 passed, 1 failed, 114 skipped",
            1,
            0,
            1,
            114,
        );
    }

    #[test]
    fn strips_ansi_color_codes() {
        let colored =
            "\x1b[1m\x1b[32mSummary\x1b[0m [73s] \x1b[1m8\x1b[0m tests run: 8 passed, 2 skipped";
        check(colored, 8, 8, 0, 2);
    }

    #[test]
    fn missing_summary_is_all_zero() {
        check("no summary line here", 0, 0, 0, 0);
    }

    #[test]
    fn takes_the_last_summary_line() {
        let log = "Summary [1s] 1 test run: 1 passed, 0 skipped\n\
                   Summary [2s] 9 tests run: 7 passed, 1 failed, 1 skipped";
        check(log, 9, 7, 1, 1);
    }
}

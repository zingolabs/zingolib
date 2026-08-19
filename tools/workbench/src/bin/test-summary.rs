//! `test-summary` runs the three test phases and prints a combined summary.
//!
//! Invoked as `makers hierarchy-test`, which runs
//! `cargo run --bin test-summary -- <nextest args>`, or as
//! `makers lite-hierarchy`, which adds `--lite` to narrow the libtonode
//! phase to the `send_shield_cycle` fixture.
//! Runs the `packages`, `zingo-cli`, and `libtonode` phases (each in its own
//! CI container via the `makers test` front door), streams each run's output
//! while capturing it, parses the nextest summary line, and aggregates the
//! totals.
//!
//! Phases gate: a failing phase stops the later, more expensive phases from
//! launching (a broken unit test should never cost a libtonode run). The
//! summary table still prints for every phase, marking the ones a failure
//! prevented from running.
//!
//! Adapted from zaino's `tools/test-runner` `live-summary` binary, which
//! instead runs all partitions unconditionally. The gating is deliberate
//! divergence.

#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

/// Whether `arg` is a cargo package-selection flag. Mirrors the
/// `has_package_selection_args` case list in Makefile.toml's base-script.
fn is_package_selection_arg(arg: &str) -> bool {
    matches!(
        arg,
        "-p" | "--package" | "--workspace" | "--all" | "--exclude" | "--manifest-path"
    ) || arg.starts_with("--package=")
        || arg.starts_with("--exclude=")
        || arg.starts_with("--manifest-path=")
}

/// The phases, in order: the hermetic package tests first, then the live
/// suites from fastest to slowest. Each entry is (display label, the
/// `makers test` invocation that selects the phase's TESTS).
///
/// Phases select tests with nextest filtersets, never with cargo package
/// selections. Every phase then shares the front door's `--workspace`
/// build scope, cargo unifies features identically across phases, and a
/// hierarchy run after the first recompiles nothing. A `-p` phase scope
/// re-unifies features per selection and compiles (then, after any source
/// change, recompiles) a distinct variant of zingolib per phase.
const PHASES: &[(&str, &str)] = &[
    ("packages", "packages"),
    ("zingo-cli", "-E 'package(zingo-cli)'"),
    ("libtonode", "-E 'package(libtonode-tests)'"),
];

/// The same phases with the libtonode one narrowed to a single fixture,
/// selected by [`LITE_FLAG`] and run as `makers lite-hierarchy`.
///
/// `send_shield_cycle` is the round trip that drives a proposal through
/// `follow_proposal` from Transmitted to Confirmed against a real LocalNet,
/// so it is the cheapest phase that still proves the chain-level machinery
/// the earlier phases cannot reach. The narrowing buys back the libtonode
/// phase's run time, which is what makes the gate usable between commits
/// rather than only before a push.
///
/// The phase also carries `--test chain_generics`, which narrows what the
/// phase BUILDS to that one of `libtonode-tests`' ten test binaries. It is
/// a cargo target selection rather than a package selection, so cargo still
/// resolves it against the whole workspace and the `--workspace` scope the
/// front door appends survives: the feature unification stays identical to
/// the other phases, and no distinct zingolib variant is compiled. On a
/// cold run this saves nothing, because the packages phase precedes it
/// under `--workspace` and has already built all ten; it saves the build
/// wherever this phase meets a tree the earlier phases did not compile.
const LITE_PHASES: &[(&str, &str)] = &[
    ("packages", "packages"),
    ("zingo-cli", "-E 'package(zingo-cli)'"),
    (
        "libtonode",
        "-E 'package(libtonode-tests) & test(chain_generics::send_shield_cycle)' \
         --test chain_generics",
    ),
];

/// This binary's own flag, consumed here and never forwarded to nextest.
const LITE_FLAG: &str = "--lite";

/// One nextest run's tallies, zero where the summary line was absent.
#[derive(Default)]
struct Summary {
    run: u64,
    passed: u64,
    failed: u64,
    timed_out: u64,
    skipped: u64,
}

impl Summary {
    fn add(&self, other: &Summary) -> Summary {
        Summary {
            run: self.run + other.run,
            passed: self.passed + other.passed,
            failed: self.failed + other.failed,
            timed_out: self.timed_out + other.timed_out,
            skipped: self.skipped + other.skipped,
        }
    }

    /// Tests the summary line counted as run but that none of the parsed
    /// terminal statuses account for (skipped is outside `run` in nextest's
    /// arithmetic). Nonzero means nextest reported a status this parser
    /// does not know, and the table would silently under-report it.
    fn unaccounted(&self) -> u64 {
        self.run
            .saturating_sub(self.passed + self.failed + self.timed_out)
    }
}

/// Run one phase through the `makers test` front door, streaming its combined
/// output to our stdout while capturing it for parsing. Returns
/// (exit_code, captured_output).
fn run_phase(invocation: &str, forwarded_args: &[String]) -> Result<(i32, String), std::io::Error> {
    // `bash -c '... 2>&1'` merges stderr into stdout so the single captured
    // stream carries the nextest summary line wherever nextest emits it.
    let mut shell_command = format!("makers test {invocation}");
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
///   Summary [1200.089s] 40 tests run: 21 passed (18 slow), 4 failed, 15 timed out, 6 skipped
fn parse_summary(log: &str) -> Summary {
    // Strip ANSI, then take the last "N test(s) run:" line nextest emitted.
    let line = log
        .lines()
        .map(strip_ansi)
        .rfind(|l| l.contains("run:") && l.contains("test"))
        .unwrap_or_default();

    Summary {
        // The run count is the integer before the word "test" ("N tests run:").
        run: count_before(&line, "test"),
        passed: count_before(&line, "passed"),
        failed: count_before(&line, "failed"),
        timed_out: count_before(&line, "timed out"),
        skipped: count_before(&line, "skipped"),
    }
}

/// Collect each non-passing test's terminal status from nextest's streamed
/// output, as (status, "suite test_name") pairs in first-seen order.
///
/// nextest prints e.g.:
///   FAIL [ 289.674s] (31/40) libtonode-tests::migration bound_note_reservation_and_external_spend_invalidation
///   TIMEOUT [ 600.017s] (40/40) libtonode-tests::migration anchorless_part_skips_without_sync
/// and re-prints the same lines in its end-of-run failure recap, so entries
/// deduplicate. `TRY n FAIL` retry lines are ignored: only a test's final
/// status line carries the plain prefix.
fn parse_failures(log: &str) -> Vec<(String, String)> {
    let mut failures: Vec<(String, String)> = Vec::new();
    for line in log.lines().map(strip_ansi) {
        let trimmed = line.trim_start();
        let status = if trimmed.starts_with("FAIL [") {
            "FAIL"
        } else if trimmed.starts_with("TIMEOUT [") {
            "TIMEOUT"
        } else {
            continue;
        };
        let Some(close_bracket) = trimmed.find(']') else {
            continue;
        };
        let rest = trimmed[close_bracket + 1..].trim_start();
        // Drop the "(31/40)" progress marker when present.
        let name = if let Some(after_paren) = rest
            .strip_prefix('(')
            .and_then(|r| r.split_once(')'))
            .map(|(_, tail)| tail)
        {
            after_paren.trim()
        } else {
            rest.trim()
        };
        if name.is_empty() {
            continue;
        }
        let entry = (status.to_string(), name.to_string());
        if !failures.contains(&entry) {
            failures.push(entry);
        }
    }
    failures
}

fn print_row(label: &str, s: &Summary) {
    println!(
        "  {label:<12} {:>4} run, {:>4} passed, {:>4} failed, {:>4} timed out, {:>4} skipped",
        s.run, s.passed, s.failed, s.timed_out, s.skipped
    );
}

fn main() -> Result<(), std::io::Error> {
    let mut forwarded_args: Vec<String> = std::env::args().skip(1).collect();

    // The lite flag chooses the phase set here; forwarding it would reach
    // nextest, which knows no such flag.
    let phases = if forwarded_args.iter().any(|arg| arg == LITE_FLAG) {
        forwarded_args.retain(|arg| arg != LITE_FLAG);
        LITE_PHASES
    } else {
        PHASES
    };

    // Forwarded args reach every phase's nextest invocation, where a
    // package selection would silently replace all three phase scopes
    // with the same one.
    if let Some(arg) = forwarded_args.iter().find(|a| is_package_selection_arg(a)) {
        eprintln!(
            "test-summary: package-selection arg '{arg}' is not accepted; each phase selects \
             its own scope. Use 'makers test -p <package>' to scope a single run."
        );
        std::process::exit(2);
    }

    let mut results = Vec::new();
    let mut failed = false;
    for (phase, invocation) in phases {
        if failed {
            results.push((*phase, None));
            continue;
        }
        println!(">>> test-summary: running the {phase} phase");
        let (exit_code, log) = run_phase(invocation, &forwarded_args)?;
        if exit_code != 0 {
            failed = true;
        }
        results.push((
            *phase,
            Some((exit_code, parse_summary(&log), parse_failures(&log))),
        ));
    }

    println!();
    println!("====================== test summary ==========================");
    let mut total = Summary::default();
    for (phase, outcome) in &results {
        match outcome {
            Some((_, summary, _)) => {
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
    // Every phase runs the same --workspace build scope filtered to its
    // own tests (one compiled variant, no per-phase feature re-unification),
    // so nextest counts the other phases' tests as skipped in each row.
    println!(
        "  note: skipped counts include the other phases' tests, since phases share one \
         --workspace build scope, filtered per phase."
    );

    // Every non-passing test by name, so nobody scrolls a 20-minute log to
    // learn what actually failed.
    for (phase, outcome) in &results {
        if let Some((_, _, failures)) = outcome {
            if !failures.is_empty() {
                println!("  {phase} non-passing tests:");
                for (status, name) in failures {
                    println!("    {status:<8} {name}");
                }
            }
        }
    }

    for (phase, outcome) in &results {
        if let Some((exit_code, summary, _)) = outcome {
            // A phase that errored without producing a summary line likely
            // failed to build; call it out so the zeros above aren't read
            // as "all clear".
            if *exit_code != 0 && summary.run == 0 {
                println!(
                    "  warning: {phase} produced no nextest summary (build failure?). See output above."
                );
            }
            // Tests counted as run but carrying a status this parser does
            // not recognize would otherwise vanish from the table.
            if summary.unaccounted() > 0 {
                println!(
                    "  warning: {phase}: {} of {} run tests carry a status test-summary does not \
                     recognize. See the nextest summary line above.",
                    summary.unaccounted(),
                    summary.run,
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
mod package_selection_guard {
    use super::*;

    #[test]
    fn rejects_every_selection_flag_form() {
        for arg in [
            "-p",
            "--package",
            "--package=zingolib",
            "--workspace",
            "--all",
            "--exclude",
            "--exclude=zingo-cli",
            "--manifest-path",
            "--manifest-path=Cargo.toml",
        ] {
            assert!(is_package_selection_arg(arg), "should reject {arg}");
        }
    }

    /// Phases must select tests (filtersets), never packages: a package
    /// selection re-unifies features per phase and compiles a distinct
    /// zingolib variant per phase, defeating the shared --workspace
    /// build scope.
    #[test]
    fn phase_invocations_select_tests_not_packages() {
        for (_, invocation) in PHASES.iter().chain(LITE_PHASES) {
            for token in invocation.split_whitespace() {
                let token = token.trim_matches('\'');
                assert!(
                    !is_package_selection_arg(token),
                    "phase invocation {invocation:?} carries package-selection arg {token:?}"
                );
            }
        }
    }

    /// HYPOTHESIS: the lite run is the full run with one phase narrowed, so
    /// it gates on everything the full run gates on except the libtonode
    /// tests it deliberately drops. Falsified if the two sets disagree on
    /// any other phase, which would let a lite run pass work the full run
    /// would have caught.
    #[test]
    fn lite_narrows_the_libtonode_phase_and_nothing_else() {
        assert_eq!(
            PHASES.len(),
            LITE_PHASES.len(),
            "a lite run must have the same phases in the same order"
        );
        for ((phase, full), (lite_phase, lite)) in PHASES.iter().zip(LITE_PHASES) {
            assert_eq!(phase, lite_phase);
            if *phase == "libtonode" {
                assert_ne!(full, lite, "the libtonode phase is the one lite narrows");
            } else {
                assert_eq!(full, lite, "lite must not touch the {phase} phase");
            }
        }
    }

    /// HYPOTHESIS: the lite phase narrows by test rather than by package, so
    /// it keeps the shared `--workspace` build scope every phase relies on.
    /// Falsified if the filterset drops the package term or the test term.
    #[test]
    fn the_lite_libtonode_phase_names_a_package_and_a_test() {
        let (_, lite) = LITE_PHASES
            .iter()
            .find(|(phase, _)| *phase == "libtonode")
            .expect("the lite set has a libtonode phase");
        assert!(lite.contains("package(libtonode-tests)"), "{lite}");
        assert!(
            lite.contains("test(chain_generics::send_shield_cycle)"),
            "{lite}"
        );
    }

    /// HYPOTHESIS: the lite phase builds one test binary rather than the
    /// ten `libtonode-tests` carries, and does so with a cargo TARGET
    /// selection, which cargo resolves workspace-wide and which therefore
    /// leaves the appended `--workspace` scope intact. Falsified if the
    /// target selection is missing, or if it is spelled as a package
    /// selection, which would re-unify features and compile a second
    /// zingolib.
    #[test]
    fn the_lite_libtonode_phase_builds_one_test_binary() {
        let (_, lite) = LITE_PHASES
            .iter()
            .find(|(phase, _)| *phase == "libtonode")
            .expect("the lite set has a libtonode phase");
        assert!(lite.contains("--test chain_generics"), "{lite}");
        assert!(
            !is_package_selection_arg("--test"),
            "--test must stay a target selection, so the front door still appends --workspace"
        );
    }

    #[test]
    fn passes_ordinary_nextest_args() {
        for arg in [
            "--no-fail-fast",
            "--no-capture",
            "-E",
            "test(slow)",
            "some_test_name",
            "--run-ignored",
        ] {
            assert!(!is_package_selection_arg(arg), "should pass {arg}");
        }
    }
}

#[cfg(test)]
mod parse_summary {
    use super::*;

    fn check(line: &str, run: u64, passed: u64, failed: u64, timed_out: u64, skipped: u64) {
        let s = parse_summary(line);
        assert_eq!(
            (s.run, s.passed, s.failed, s.timed_out, s.skipped),
            (run, passed, failed, timed_out, skipped)
        );
    }

    #[test]
    fn plural_no_failures() {
        check(
            "Summary [ 73.207s] 8 tests run: 8 passed (2 slow), 2 skipped",
            8,
            8,
            0,
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
            0,
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
            0,
            114,
        );
    }

    /// The 2026-07-15 container run: fifteen timeouts a passed/failed/skipped
    /// parse silently dropped from the table.
    #[test]
    fn timeouts_are_counted() {
        let line = "Summary [1200.089s] 40 tests run: 21 passed (18 slow), 4 failed, 15 timed out, 6 skipped";
        check(line, 40, 21, 4, 15, 6);
        assert_eq!(parse_summary(line).unaccounted(), 0);
    }

    #[test]
    fn unrecognized_statuses_are_unaccounted() {
        // A hypothetical status keyword this parser does not know.
        let line = "Summary [10s] 5 tests run: 3 passed, 2 vaporized, 0 skipped";
        assert_eq!(parse_summary(line).unaccounted(), 2);
    }

    #[test]
    fn strips_ansi_color_codes() {
        let colored =
            "\x1b[1m\x1b[32mSummary\x1b[0m [73s] \x1b[1m8\x1b[0m tests run: 8 passed, 2 skipped";
        check(colored, 8, 8, 0, 0, 2);
    }

    #[test]
    fn missing_summary_is_all_zero() {
        check("no summary line here", 0, 0, 0, 0, 0);
    }

    #[test]
    fn takes_the_last_summary_line() {
        let log = "Summary [1s] 1 test run: 1 passed, 0 skipped\n\
                   Summary [2s] 9 tests run: 7 passed, 1 failed, 1 skipped";
        check(log, 9, 7, 1, 0, 1);
    }
}

#[cfg(test)]
mod parse_failures {
    use super::*;

    /// Real lines from the 2026-07-15 container run: streamed FAIL and
    /// TIMEOUT statuses, the end-of-run recap re-printing one of them, a
    /// retry line, and a PASS line. Only final non-passing statuses
    /// survive, once each.
    #[test]
    fn collects_and_deduplicates_terminal_statuses() {
        let log = "        PASS [  50.205s] ( 1/40) libtonode-tests::concrete mine_to_transparent\n\
             \x20       FAIL [ 289.674s] (31/40) libtonode-tests::migration bound_note_reservation_and_external_spend_invalidation\n\
             \x20    TIMEOUT [ 600.017s] (40/40) libtonode-tests::migration anchorless_part_skips_without_sync\n\
             \x20   TRY 2 FAIL [  1.002s] ( 2/40) libtonode-tests::concrete retried_test\n\
             \x20       FAIL [ 289.674s] (31/40) libtonode-tests::migration bound_note_reservation_and_external_spend_invalidation\n";
        assert_eq!(
            parse_failures(log),
            vec![
                (
                    "FAIL".to_string(),
                    "libtonode-tests::migration bound_note_reservation_and_external_spend_invalidation"
                        .to_string()
                ),
                (
                    "TIMEOUT".to_string(),
                    "libtonode-tests::migration anchorless_part_skips_without_sync"
                        .to_string()
                ),
            ]
        );
    }

    #[test]
    fn survives_ansi_and_missing_progress_marker() {
        let log = "\x1b[1m\x1b[31mFAIL\x1b[0m [ 17.210s] libtonode-tests::sync store_all_checkpoints_in_verification_window\n";
        assert_eq!(
            parse_failures(log),
            vec![(
                "FAIL".to_string(),
                "libtonode-tests::sync store_all_checkpoints_in_verification_window".to_string()
            )]
        );
    }

    #[test]
    fn clean_run_yields_nothing() {
        assert!(parse_failures("PASS [ 1s] (1/1) suite test_one\nSummary ...").is_empty());
    }
}

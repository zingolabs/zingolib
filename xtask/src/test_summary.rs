use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::test;

/// The phases, in order: the hermetic package tests first, then the live
/// suites from fastest to slowest. Each entry is (display label, the
/// `cargo xtask test` arguments that select the phase's TESTS).
///
/// Phases select tests with nextest filtersets, never with cargo package
/// selections. Every phase then shares the front door's `--workspace`
/// build scope, cargo unifies features identically across phases, and a
/// hierarchy run after the first recompiles nothing. A `-p` phase scope
/// re-unifies features per selection and compiles (then, after any source
/// change, recompiles) a distinct variant of zingolib per phase.
const PHASES: &[(&str, &[&str])] = &[
    ("packages", &[test::PACKAGES_WORD]),
    ("zingo-cli", &["-E", "package(zingo-cli)"]),
    ("libtonode", &["-E", "package(libtonode-tests)"]),
];

/// The same phases with the libtonode one narrowed to a single fixture,
/// selected by [`LITE_FLAG`] and run as `cargo xtask lite-hierarchy`.
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
const LITE_PHASES: &[(&str, &[&str])] = &[
    ("packages", &[test::PACKAGES_WORD]),
    ("zingo-cli", &["-E", "package(zingo-cli)"]),
    (
        "libtonode",
        &[
            "-E",
            "package(libtonode-tests) & test(chain_generics::send_shield_cycle)",
            "--test",
            "chain_generics",
        ],
    ),
];

pub const LITE_FLAG: &str = "--lite";

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

/// - Spawns the containerized nextest run of one phase through [`test::container_command`] and waits for it.
/// - Writes the run's merged stdout and stderr to stdout, line by line.
fn run_phase(
    root: &Path,
    invocation: &[&str],
    forwarded_args: &[String],
) -> Result<(i32, String), Vec<String>> {
    let args: Vec<String> = invocation
        .iter()
        .map(ToString::to_string)
        .chain(forwarded_args.iter().cloned())
        .collect();
    let mut command = test::container_command(root, &test::front_door_args(&args))?;

    let (reader, writer) = std::io::pipe().map_err(|e| vec![format!("cannot open a pipe: {e}")])?;
    let stderr = writer
        .try_clone()
        .map_err(|e| vec![format!("cannot clone the pipe: {e}")])?;
    command.stdout(writer).stderr(stderr);
    let mut child = command
        .spawn()
        .map_err(|e| vec![format!("cannot spawn the test run: {e}")])?;
    drop(command);

    let mut captured = String::new();
    for line in BufReader::new(reader).lines() {
        let line = line.map_err(|e| vec![format!("cannot read the test run: {e}")])?;
        println!("{line}");
        captured.push_str(&line);
        captured.push('\n');
    }

    let code = child
        .wait()
        .map_err(|e| vec![format!("cannot wait for the test run: {e}")])?
        .code()
        .unwrap_or(1);
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

/// - Runs each phase through [`run_phase`], stopping after a failing one.
/// - Writes the summary table to stdout, and exits this process with 1 when a phase failed.
pub fn dispatch(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    let mut forwarded_args: Vec<String> = args.to_vec();

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
    if let Some(arg) = forwarded_args
        .iter()
        .find(|arg| test::is_package_selection(arg))
    {
        return Err(vec![format!(
            "package-selection arg '{arg}' is not accepted; each phase selects its own scope. \
             Use 'cargo xtask test -p <package>' to scope a single run."
        )]);
    }

    let mut results = Vec::new();
    let mut failed = false;
    for (phase, invocation) in phases {
        if failed {
            results.push((*phase, None));
            continue;
        }
        println!(">>> test-summary: running the {phase} phase");
        let (exit_code, log) = run_phase(root, invocation, &forwarded_args)?;
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
        if let Some((_, _, failures)) = outcome
            && !failures.is_empty()
        {
            println!("  {phase} non-passing tests:");
            for (status, name) in failures {
                println!("    {status:<8} {name}");
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

    /// Phases must select tests (filtersets), never packages: a package
    /// selection re-unifies features per phase and compiles a distinct
    /// zingolib variant per phase, defeating the shared --workspace
    /// build scope.
    #[test]
    fn phase_invocations_select_tests_not_packages() {
        for (_, invocation) in PHASES.iter().chain(LITE_PHASES) {
            for token in *invocation {
                assert!(
                    !test::is_package_selection(token),
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
        let filterset = lite[1];
        assert!(filterset.contains("package(libtonode-tests)"), "{lite:?}");
        assert!(
            filterset.contains("test(chain_generics::send_shield_cycle)"),
            "{lite:?}"
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
        assert!(
            lite.windows(2)
                .any(|pair| pair == ["--test", "chain_generics"]),
            "{lite:?}"
        );
        assert!(
            !test::is_package_selection("--test"),
            "--test must stay a target selection, so the front door still appends --workspace"
        );
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

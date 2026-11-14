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
    let line =
        "Summary [1200.089s] 40 tests run: 21 passed (18 slow), 4 failed, 15 timed out, 6 skipped";
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

use std::path::Path;

use super::*;

const GATE_WORKFLOW: &str = ".github/workflows/no-duplicated-code.yaml";
const PULL_REQUEST_WORKFLOW: &str = ".github/workflows/ci-pr.yaml";
const GATE_JOB: &str = "no-duplicated-code";
const CHECK_NAME: &str = "No duplicated code";
const FINGERPRINT_MARK: &str = "fingerprint: ";
const PRUNED_DIRECTORY: &str = "target";
const HIDDEN_MARK: char = '.';
const DIRECTORY_MARK: char = '/';
const FIXTURE_ROOT: &str = "dupes-gate";
const NO_NEAR: &str = "0";
const SLACK_CEILING: &str = "1";
const SOURCE: &str = "src/lib.rs";
const NESTED_CHECKOUT: &str = "testing/checkout";
const PARENT_PATTERN: &str = "/testing/";
const STALE_BASELINE: &str = "[[ignore]]\nfingerprint = \"0123456789abcdef\"\n";
const TOOL_ERROR: i32 = 2;
const STALE_ENTRIES: &str = "Stale entries (dry run)";
const NEAR_GROUP: &str = "near duplicate groups";
const EXACT_GROUP: &str = "exact duplicate groups";

fn fixture(name: &str, files: &[(&str, &str)]) -> Result<crate::ScratchDir, Vec<String>> {
    let dir = crate::scratch_dir(&format!("{FIXTURE_ROOT}-{name}"))?;
    for (relative, contents) in files {
        let file = dir.join(relative);
        crate::create_parent(&file)?;
        crate::write(&file, contents)?;
    }
    Ok(dir)
}

fn is_excluded(path: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|pattern| path.contains(pattern))
}

fn is_pruned(pattern: &str) -> bool {
    Path::new(pattern).components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        name == PRUNED_DIRECTORY || name.starts_with(HIDDEN_MARK)
    })
}

fn fingerprints(report: &str) -> Vec<String> {
    report
        .match_indices(FINGERPRINT_MARK)
        .map(|(at, mark)| {
            report[at + mark.len()..]
                .chars()
                .take_while(|c| c.is_ascii_hexdigit())
                .collect()
        })
        .collect()
}

fn function(name: &str, tail: &str) -> String {
    format!(
        "fn {name}(input: &[u64]) -> u64 {{
    let mut total = 0u64;
    for value in input.iter() {{
        if *value > 2 {{
            total += *value * 3;
        }} else {{
            total -= 1;
        }}
    }}
    {tail}
    total
}}
"
    )
}

fn functions(tails: &[(&str, &str)]) -> String {
    tails
        .iter()
        .map(|(name, tail)| function(name, tail))
        .collect()
}

fn lone() -> String {
    function("only", "")
}

fn pair(attribute: &str) -> String {
    format!(
        "{attribute}\n{}{attribute}\n{}",
        function("first", ""),
        function("second", "")
    )
}

fn module(attribute: &str, body: &str) -> String {
    format!("{attribute}\nmod tests {{\n{body}}}\n")
}

fn near_group(third_tail: &str) -> String {
    functions(&[
        ("first", "total += 1;"),
        ("second", "total -= 1;"),
        ("third", third_tail),
    ])
}

fn stale(name: &str) -> Result<crate::ScratchDir, Vec<String>> {
    fixture(name, &[(SOURCE, &lone()), (BASELINE, STALE_BASELINE)])
}

fn baseline(dir: &Path) -> Result<(), Vec<String>> {
    let report = dupes(dir, &["report"])?.stdout;
    for fingerprint in fingerprints(&report) {
        dupes(dir, &["ignore", &fingerprint])?;
    }
    assert_eq!(check(dir, NO_NEAR), Ok(()));
    Ok(())
}

fn after_rewrite(
    name: &str,
    before: &str,
    after: &str,
) -> Result<(crate::Finished, String), Vec<String>> {
    let checkout = fixture(name, &[(SOURCE, before)])?;
    baseline(&checkout)?;
    crate::write(&checkout.join(SOURCE), after)?;
    Ok((
        dupes(&checkout, &check_args(NO_NEAR))?,
        dupes(&checkout, &CLEANUP)?.stdout,
    ))
}

fn read_root(relative: &str) -> Result<String, Vec<String>> {
    crate::read(&crate::repo_root()?.join(relative))
}

fn tracked_rust_files() -> Result<Vec<String>, Vec<String>> {
    let root = crate::repo_root()?;
    Ok(crate::git_in(&root, &["ls-files", "--", "*.rs"])?
        .lines()
        .map(|file| root.join(file).to_string_lossy().into_owned())
        .collect())
}

#[test]
fn every_exclude_pattern_matches_a_tracked_rust_file() -> Result<(), Vec<String>> {
    let files = tracked_rust_files()?;
    for pattern in EXCLUDE {
        assert!(
            files.iter().any(|file| file.contains(pattern)),
            "{pattern} matches no tracked Rust file"
        );
    }
    Ok(())
}

#[test]
fn no_exclude_pattern_names_a_directory_the_scanner_prunes() {
    for pattern in EXCLUDE {
        assert!(!is_pruned(pattern), "{pattern} is pruned without it");
    }
}

#[test]
fn every_directory_pattern_is_a_path_from_the_repository_root() -> Result<(), Vec<String>> {
    let root = crate::repo_root()?;
    for pattern in EXCLUDE {
        if pattern.ends_with(DIRECTORY_MARK) {
            assert!(
                root.join(pattern).is_dir(),
                "{pattern} is not a directory under the root"
            );
        }
    }
    Ok(())
}

#[test]
fn no_pattern_matches_a_parent_directory_or_a_longer_name() {
    let checkout = "/home/someone/testing/target/tests/zls";
    for production in [
        "crates/zingolib/src/lib.rs",
        "crates/zingolib/src/requests.rs",
        "bin/cli/src/attesting.rs",
        "crates/pepper-sync/src/stocks.rs",
    ] {
        let path = format!("{checkout}/{production}");
        assert!(!is_excluded(&path, &EXCLUDE), "{path} is excluded");
    }
    for test in [
        "ffi/lib/src/lock_discipline_tests.rs",
        "crates/zingolib/src/wallet/disk/testing.rs",
        "bin/cli/src/tests.rs",
    ] {
        let path = format!("{checkout}/{test}");
        assert!(is_excluded(&path, &EXCLUDE), "{path} is not excluded");
    }
}

#[test]
fn the_gate_workflow_filters_no_path() -> Result<(), Vec<String>> {
    let workflow = read_root(GATE_WORKFLOW)?;
    let keys: Vec<&str> = workflow.lines().map(str::trim).collect();
    assert!(keys.contains(&"pull_request:"));
    assert!(!keys.contains(&"paths:"));
    assert!(!keys.contains(&"paths-ignore:"));
    Ok(())
}

#[test]
fn a_job_in_the_gate_workflow_reports_the_required_check_name() -> Result<(), Vec<String>> {
    let name = format!("name: {CHECK_NAME}");
    assert!(read_root(GATE_WORKFLOW)?
        .lines()
        .any(|line| line.starts_with(char::is_whitespace) && line.trim() == name));
    Ok(())
}

#[test]
fn the_pull_request_workflow_carries_no_gate() -> Result<(), Vec<String>> {
    let workflow = read_root(PULL_REQUEST_WORKFLOW)?;
    assert!(!workflow.contains(GATE_JOB));
    assert!(!workflow.contains(SUBCOMMAND));
    Ok(())
}

#[test]
fn the_gate_passes_on_this_checkout() -> Result<(), Vec<String>> {
    assert_eq!(gate(&crate::repo_root()?), Ok(()));
    Ok(())
}

#[test]
fn a_near_count_below_the_ceiling_is_rejected() -> Result<(), Vec<String>> {
    let checkout = fixture("slack-ceiling", &[(SOURCE, &lone())])?;
    assert_eq!(
        check(&checkout, SLACK_CEILING),
        Err(Rejection::NearBelowCeiling {
            count: NO_NEAR.to_string(),
            ceiling: SLACK_CEILING.to_string(),
        })
    );
    assert_eq!(check(&checkout, NO_NEAR), Ok(()));
    Ok(())
}

#[test]
fn a_tool_error_is_not_reported_as_duplication() -> Result<(), Vec<String>> {
    let empty = fixture("check-empty", &[("README.md", "")])?;
    assert!(matches!(
        check(&empty, NO_NEAR),
        Err(Rejection::ToolFailed(_))
    ));
    assert_eq!(
        check(&fixture("check-pair", &[(SOURCE, &pair(""))])?, NO_NEAR),
        Err(Rejection::Duplication)
    );
    Ok(())
}

#[test]
fn an_async_test_in_a_feature_gated_module_is_not_excluded() -> Result<(), Vec<String>> {
    let body = pair("#[tokio::test]");
    let checkout = fixture(
        "async-gated",
        &[(
            SOURCE,
            &module("#[cfg(any(test, feature = \"test-elevation\"))]", &body),
        )],
    )?;
    let run = dupes(&checkout, &check_args(NO_NEAR))?;
    assert_eq!(run.status.code(), Some(DUPLICATION));
    assert!(run.stdout.contains(EXACT_GROUP));
    Ok(())
}

#[test]
fn a_literal_test_in_a_cfg_test_module_is_excluded() -> Result<(), Vec<String>> {
    let body = pair("#[test]");
    let checkout = fixture("literal-test", &[(SOURCE, &module("#[cfg(test)]", &body))])?;
    assert_eq!(check(&checkout, NO_NEAR), Ok(()));
    Ok(())
}

#[test]
fn a_near_group_gets_a_new_fingerprint_when_a_member_changes() -> Result<(), Vec<String>> {
    let (check, cleanup) = after_rewrite(
        "near-edit",
        &near_group("total *= 2;"),
        &near_group("total ^= 2;"),
    )?;
    assert_eq!(check.status.code(), Some(DUPLICATION));
    assert!(check.stdout.contains(NEAR_GROUP));
    assert!(cleanup.contains(STALE_ENTRIES));
    Ok(())
}

#[test]
fn a_near_group_gets_a_new_fingerprint_when_a_member_leaves() -> Result<(), Vec<String>> {
    let (check, cleanup) = after_rewrite(
        "near-leave",
        &near_group("total *= 2;"),
        &functions(&[("first", "total += 1;"), ("second", "total -= 1;")]),
    )?;
    assert_eq!(check.status.code(), Some(DUPLICATION));
    assert!(check.stdout.contains(NEAR_GROUP));
    assert!(cleanup.contains(STALE_ENTRIES));
    Ok(())
}

#[test]
fn an_exact_group_keeps_its_fingerprint_when_a_member_leaves() -> Result<(), Vec<String>> {
    let (check, cleanup) = after_rewrite(
        "exact-leave",
        &functions(&[("first", ""), ("second", ""), ("third", "")]),
        &functions(&[("first", ""), ("second", "")]),
    )?;
    assert_eq!(check.status.code(), Some(PASSED));
    assert!(cleanup.contains(NO_STALE_ENTRIES));
    Ok(())
}

#[test]
fn a_target_directory_is_pruned_without_a_pattern() -> Result<(), Vec<String>> {
    let checkout = fixture(
        "pruned",
        &[
            (SOURCE, &lone()),
            (&format!("{PRUNED_DIRECTORY}/generated.rs"), &pair("")),
        ],
    )?;
    assert_eq!(check(&checkout, NO_NEAR), Ok(()));
    Ok(())
}

#[test]
fn a_pattern_matches_a_parent_directory_of_the_checkout() -> Result<(), Vec<String>> {
    let parent = fixture(
        "parent",
        &[(&format!("{NESTED_CHECKOUT}/{SOURCE}"), &lone())],
    )?;
    let checkout = parent.join(NESTED_CHECKOUT);
    assert_eq!(check(&checkout, NO_NEAR), Ok(()));
    let args: Vec<&str> = check_args(NO_NEAR)
        .into_iter()
        .chain([EXCLUDE_FLAG, PARENT_PATTERN])
        .collect();
    assert_eq!(dupes(&checkout, &args)?.status.code(), Some(TOOL_ERROR));
    Ok(())
}

#[test]
fn cleanup_exits_zero_with_a_stale_entry_and_two_without_sources() -> Result<(), Vec<String>> {
    let run = dupes(&stale("stale")?, &CLEANUP)?;
    assert_eq!(run.status.code(), Some(PASSED));
    assert!(run.stdout.contains(STALE_ENTRIES));
    let empty = fixture("empty", &[("README.md", "")])?;
    assert_eq!(dupes(&empty, &CLEANUP)?.status.code(), Some(TOOL_ERROR));
    Ok(())
}

#[test]
fn the_gate_names_a_stale_entry() -> Result<(), Vec<String>> {
    assert_eq!(
        no_stale_entry(&stale("gate-stale")?),
        Err(Rejection::StaleEntry)
    );
    let clean = fixture("gate-clean", &[(SOURCE, &lone())])?;
    assert_eq!(no_stale_entry(&clean), Ok(()));
    Ok(())
}

#[test]
fn the_near_count_is_read_from_a_json_report() {
    let report = "{\n  \"exact_duplicate_groups\": 0,\n  \"near_duplicate_groups\": 36,\n  \"near_duplicate_units\": 95\n}\n\nCheck passed.\n";
    assert_eq!(near_count(report), Some("36"));
    assert_eq!(near_count("Check passed.\n"), None);
}

#[test]
fn fingerprints_are_read_from_a_report() {
    let report =
        "Group 1 (fingerprint: 0a1b, 2 members)\nGroup 2 (fingerprint: ffff, similarity: 97%)";
    assert_eq!(fingerprints(report), ["0a1b", "ffff"]);
}

use std::path::Path;

pub const BINARY: &str = "dupes-gate";

const BASELINE: &str = ".dupes-ignore.toml";
const VERSION: &str = "0.2.1";
const MIN_NODES: &str = "10";
const MIN_LINES: &str = "0";
const THRESHOLD: &str = "0.9";
const MAX_EXACT: &str = "0";
const MAX_NEAR: &str = "36";
const EXCLUDE: [&str; 12] = [
    "libtonode-tests/",
    "zingolib_testutils/",
    "zingo-cli/tests/",
    "zingo-netutils/nym-proxy-ffi/tests/",
    "zingolib/src/testutils/",
    "zingolib/src/wallet/disk/testing/",
    "zingolib/tests/",
    "/mocks.rs",
    "/testing.rs",
    "/tests.rs",
    "/testutils.rs",
    "_tests.rs",
];

const SUBCOMMAND: &str = "dupes";
const INSTALL_FLAG: &str = "--install";
const EXCLUDE_FLAG: &str = "--exclude";
const SETTINGS: [&str; 7] = [
    "--min-nodes",
    MIN_NODES,
    "--min-lines",
    MIN_LINES,
    "--threshold",
    THRESHOLD,
    "--exclude-tests",
];
const CLEANUP_SUBCOMMAND: &str = "cleanup";
const CLEANUP: [&str; 2] = [CLEANUP_SUBCOMMAND, "--dry-run"];
const NO_STALE_ENTRIES: &str = "No stale entries found";
const NEAR_COUNT_KEY: &str = "\"near_duplicate_groups\":";
const VALUE_END: char = ',';
const RUSTFLAGS: &str = "RUSTFLAGS";
const NO_FLAGS: &str = "";
const PASSED: i32 = 0;
const DUPLICATION: i32 = 1;

#[derive(Debug, PartialEq, Eq)]
enum Rejection {
    Unrunnable(Vec<String>),
    NotPinned(String),
    ToolFailed(String),
    Duplication,
    NearBelowCeiling { count: String, ceiling: String },
    NoNearCount,
    StaleEntry,
}

impl From<Vec<String>> for Rejection {
    fn from(lines: Vec<String>) -> Self {
        Rejection::Unrunnable(lines)
    }
}

impl Rejection {
    fn lines(self) -> Vec<String> {
        match self {
            Rejection::Unrunnable(lines) => lines,
            Rejection::NotPinned(installed) => vec![format!(
                "{installed} is installed and the gate pins cargo-{SUBCOMMAND} {VERSION}; run `{}`",
                gate_command(INSTALL_FLAG)
            )],
            Rejection::ToolFailed(failure) => vec![failure],
            Rejection::Duplication => vec![format!(
                "an exact group lies outside {BASELINE}, or the near groups exceed the ceiling; \
                 remove the duplication and never add an entry (AGENTS.md)"
            )],
            Rejection::NearBelowCeiling { count, ceiling } => vec![format!(
                "{count} near groups exist under a ceiling of {ceiling}; \
                 lower MAX_NEAR to {count} (AGENTS.md)"
            )],
            Rejection::NoNearCount => {
                vec![format!("the check report holds no {NEAR_COUNT_KEY} line")]
            }
            Rejection::StaleEntry => vec![format!(
                "{BASELINE} holds a stale entry; run `{}` and commit the result",
                gate_command(CLEANUP_SUBCOMMAND)
            )],
        }
    }
}

fn gate_command(args: &str) -> String {
    format!(
        "{} run --manifest-path {}/{} --bin {BINARY} -- {args}",
        crate::CARGO,
        crate::WORKBENCH_RELATIVE_DIR,
        crate::MANIFEST
    )
}

fn settings() -> Vec<&'static str> {
    SETTINGS
        .into_iter()
        .chain(
            EXCLUDE
                .into_iter()
                .flat_map(|pattern| [EXCLUDE_FLAG, pattern]),
        )
        .collect()
}

fn check_args(max_near: &str) -> [&str; 7] {
    [
        "check",
        "--max-exact",
        MAX_EXACT,
        "--max-near",
        max_near,
        "--format",
        "json",
    ]
}

fn near_count(report: &str) -> Option<&str> {
    report
        .lines()
        .find_map(|line| line.trim().strip_prefix(NEAR_COUNT_KEY))
        .map(|value| value.trim().trim_end_matches(VALUE_END))
}

fn tool_failed(args: &[&str], finished: &crate::Finished) -> Rejection {
    Rejection::ToolFailed(format!(
        "`{} {SUBCOMMAND} {}` failed ({})",
        crate::CARGO,
        args.join(" "),
        finished.status
    ))
}

/// - Runs `cargo dupes <args>` with the gate's settings as a child process in `dir`, with stderr inherited, and waits for it.
fn dupes(dir: &Path, args: &[&str]) -> Result<crate::Finished, Vec<String>> {
    let mut command = vec![SUBCOMMAND];
    command.extend(args);
    command.extend(settings());
    crate::finished_in(dir, crate::CARGO, &command, &[])
}

/// - Runs `cargo dupes <args>` with the gate's settings as a child process in `dir` and waits for it.
/// - Prints the child's stdout to stdout.
fn printed(dir: &Path, args: &[&str]) -> Result<crate::Finished, Vec<String>> {
    let finished = dupes(dir, args)?;
    print!("{}", finished.stdout);
    Ok(finished)
}

/// - Runs `cargo dupes --version` as a child process and waits for it.
fn pinned() -> Result<(), Rejection> {
    let installed = crate::cargo_subcommand_version(SUBCOMMAND, &gate_command(INSTALL_FLAG))?;
    if installed == format!("cargo-{SUBCOMMAND} {VERSION}") {
        Ok(())
    } else {
        Err(Rejection::NotPinned(installed))
    }
}

/// - Runs `cargo dupes check` in `dir` and prints its report to stdout.
fn check(dir: &Path, max_near: &str) -> Result<(), Rejection> {
    let args = check_args(max_near);
    let finished = printed(dir, &args)?;
    match finished.status.code() {
        Some(PASSED) => {}
        Some(DUPLICATION) => return Err(Rejection::Duplication),
        _ => return Err(tool_failed(&args, &finished)),
    }
    match near_count(&finished.stdout) {
        Some(count) if count == max_near => Ok(()),
        Some(count) => Err(Rejection::NearBelowCeiling {
            count: count.to_string(),
            ceiling: max_near.to_string(),
        }),
        None => Err(Rejection::NoNearCount),
    }
}

/// - Runs `cargo dupes cleanup --dry-run` in `dir` and prints its report to stdout.
fn no_stale_entry(dir: &Path) -> Result<(), Rejection> {
    let finished = printed(dir, &CLEANUP)?;
    if !finished.status.success() {
        return Err(tool_failed(&CLEANUP, &finished));
    }
    if finished.stdout.contains(NO_STALE_ENTRIES) {
        Ok(())
    } else {
        Err(Rejection::StaleEntry)
    }
}

/// - Runs `cargo dupes --version`, `cargo dupes check`, and `cargo dupes cleanup --dry-run` in `root`.
/// - Prints the check report and the cleanup report to stdout.
fn gate(root: &Path) -> Result<(), Rejection> {
    pinned()?;
    check(root, MAX_NEAR)?;
    no_stale_entry(root)
}

/// - Runs `cargo install` for the pinned cargo-dupes with empty `RUSTFLAGS`, which writes the binary under the cargo home.
fn install(root: &Path) -> Result<(), Rejection> {
    let package = format!("cargo-{SUBCOMMAND}");
    crate::run_streaming_in(
        root,
        crate::CARGO,
        &["install", &package, "--version", VERSION, "--locked"],
        &[(RUSTFLAGS, NO_FLAGS)],
    )?;
    Ok(())
}

/// - Runs `cargo dupes <args>` in `root` and prints its stdout, which lets `cleanup` and `ignore` rewrite the baseline.
fn forward(root: &Path, args: &[String]) -> Result<(), Rejection> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let finished = printed(root, &args)?;
    if finished.status.success() {
        Ok(())
    } else {
        Err(tool_failed(&args, &finished))
    }
}

/// - Runs `cargo dupes` child processes in `root`, or `cargo install` for the pinned cargo-dupes when `args` is `--install`.
/// - Prints each cargo-dupes report to stdout.
pub fn dispatch(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    match args {
        [] => gate(root),
        [flag] if flag == INSTALL_FLAG => install(root),
        _ => forward(root, args),
    }
    .map_err(Rejection::lines)
}

/// - Reads the process arguments.
/// - Exits the process through [`crate::run`].
pub fn main() -> ! {
    crate::dispatch_from_root(BINARY, dispatch)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    const GATE_WORKFLOW: &str = ".github/workflows/no-duplicated-code.yaml";
    const PULL_REQUEST_WORKFLOW: &str = ".github/workflows/ci-pr.yaml";
    const GATE_JOB: &str = "no-duplicated-code";
    const CHECK_NAME: &str = "No duplicated code";
    const FINGERPRINT_MARK: &str = "fingerprint: ";
    const PRUNED_DIRECTORY: &str = "target";
    const HIDDEN_MARK: char = '.';
    const DIRECTORY_MARK: char = '/';
    const FIXTURE_ROOT: &str = "workbench-dupes-gate";
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

    struct Fixture {
        dir: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(name: &str, files: &[(&str, &str)]) -> Fixture {
        let dir = std::env::temp_dir()
            .join(FIXTURE_ROOT)
            .join(format!("{name}-{}", std::process::id()));
        crate::fresh_dir(&dir).unwrap();
        for (relative, contents) in files {
            let file = dir.join(relative);
            crate::create_parent(&file).unwrap();
            std::fs::write(&file, contents).unwrap();
        }
        Fixture { dir }
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

    fn tool(dir: &Path, args: &[&str]) -> crate::Finished {
        dupes(dir, args).unwrap()
    }

    fn strict(dir: &Path) -> crate::Finished {
        tool(dir, &check_args(NO_NEAR))
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

    fn stale(name: &str) -> Fixture {
        fixture(name, &[(SOURCE, &lone()), (BASELINE, STALE_BASELINE)])
    }

    fn baseline(dir: &Path) {
        let report = tool(dir, &["report"]).stdout;
        for fingerprint in fingerprints(&report) {
            tool(dir, &["ignore", &fingerprint]);
        }
        assert_eq!(check(dir, NO_NEAR), Ok(()));
    }

    fn after_rewrite(name: &str, before: &str, after: &str) -> (crate::Finished, String) {
        let checkout = fixture(name, &[(SOURCE, before)]);
        baseline(&checkout.dir);
        std::fs::write(checkout.dir.join(SOURCE), after).unwrap();
        (strict(&checkout.dir), tool(&checkout.dir, &CLEANUP).stdout)
    }

    fn root() -> PathBuf {
        crate::repo_root().unwrap()
    }

    fn read_root(relative: &str) -> String {
        crate::read(&root().join(relative)).unwrap()
    }

    fn tracked_rust_files() -> Vec<String> {
        let root = root();
        crate::git_in(&root, &["ls-files", "--", "*.rs"])
            .unwrap()
            .lines()
            .map(|file| root.join(file).to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn every_exclude_pattern_matches_a_tracked_rust_file() {
        let files = tracked_rust_files();
        for pattern in EXCLUDE {
            assert!(
                files.iter().any(|file| file.contains(pattern)),
                "{pattern} matches no tracked Rust file"
            );
        }
    }

    #[test]
    fn no_exclude_pattern_names_a_directory_the_scanner_prunes() {
        for pattern in EXCLUDE {
            assert!(!is_pruned(pattern), "{pattern} is pruned without it");
        }
    }

    #[test]
    fn every_directory_pattern_is_a_path_from_the_repository_root() {
        for pattern in EXCLUDE {
            if pattern.ends_with(DIRECTORY_MARK) {
                assert!(
                    root().join(pattern).is_dir(),
                    "{pattern} is not a directory under the root"
                );
            }
        }
    }

    #[test]
    fn no_pattern_matches_a_parent_directory_or_a_longer_name() {
        let checkout = "/home/someone/testing/target/tests/zls";
        for production in [
            "zingolib/src/lib.rs",
            "zingolib/src/requests.rs",
            "zingo-cli/src/attesting.rs",
            "pepper-sync/src/stocks.rs",
        ] {
            let path = format!("{checkout}/{production}");
            assert!(!is_excluded(&path, &EXCLUDE), "{path} is excluded");
        }
        for test in [
            "zingo-ffi/lib/src/lock_discipline_tests.rs",
            "zingolib/src/wallet/disk/testing.rs",
            "zingo-cli/src/tests.rs",
        ] {
            let path = format!("{checkout}/{test}");
            assert!(is_excluded(&path, &EXCLUDE), "{path} is not excluded");
        }
    }

    #[test]
    fn the_gate_workflow_filters_no_path() {
        let workflow = read_root(GATE_WORKFLOW);
        let keys: Vec<&str> = workflow.lines().map(str::trim).collect();
        assert!(keys.contains(&"pull_request:"));
        assert!(!keys.contains(&"paths:"));
        assert!(!keys.contains(&"paths-ignore:"));
    }

    #[test]
    fn a_job_in_the_gate_workflow_reports_the_required_check_name() {
        let name = format!("name: {CHECK_NAME}");
        assert!(read_root(GATE_WORKFLOW)
            .lines()
            .any(|line| line.starts_with(char::is_whitespace) && line.trim() == name));
    }

    #[test]
    fn the_pull_request_workflow_carries_no_gate() {
        let workflow = read_root(PULL_REQUEST_WORKFLOW);
        assert!(!workflow.contains(GATE_JOB));
        assert!(!workflow.contains(SUBCOMMAND));
    }

    #[test]
    fn the_gate_passes_on_this_checkout() {
        assert_eq!(gate(&root()), Ok(()));
    }

    #[test]
    fn a_near_count_below_the_ceiling_is_rejected() {
        let checkout = fixture("slack-ceiling", &[(SOURCE, &lone())]);
        assert_eq!(
            check(&checkout.dir, SLACK_CEILING),
            Err(Rejection::NearBelowCeiling {
                count: NO_NEAR.to_string(),
                ceiling: SLACK_CEILING.to_string(),
            })
        );
        assert_eq!(check(&checkout.dir, NO_NEAR), Ok(()));
    }

    #[test]
    fn a_tool_error_is_not_reported_as_duplication() {
        let empty = fixture("check-empty", &[("README.md", "")]);
        assert!(matches!(
            check(&empty.dir, NO_NEAR),
            Err(Rejection::ToolFailed(_))
        ));
        assert_eq!(
            check(&fixture("check-pair", &[(SOURCE, &pair(""))]).dir, NO_NEAR),
            Err(Rejection::Duplication)
        );
    }

    #[test]
    fn an_async_test_in_a_feature_gated_module_is_not_excluded() {
        let body = pair("#[tokio::test]");
        let checkout = fixture(
            "async-gated",
            &[(
                SOURCE,
                &module("#[cfg(any(test, feature = \"test-elevation\"))]", &body),
            )],
        );
        let run = strict(&checkout.dir);
        assert_eq!(run.status.code(), Some(DUPLICATION));
        assert!(run.stdout.contains(EXACT_GROUP));
    }

    #[test]
    fn a_literal_test_in_a_cfg_test_module_is_excluded() {
        let body = pair("#[test]");
        let checkout = fixture("literal-test", &[(SOURCE, &module("#[cfg(test)]", &body))]);
        assert_eq!(check(&checkout.dir, NO_NEAR), Ok(()));
    }

    #[test]
    fn a_near_group_gets_a_new_fingerprint_when_a_member_changes() {
        let (check, cleanup) = after_rewrite(
            "near-edit",
            &near_group("total *= 2;"),
            &near_group("total ^= 2;"),
        );
        assert_eq!(check.status.code(), Some(DUPLICATION));
        assert!(check.stdout.contains(NEAR_GROUP));
        assert!(cleanup.contains(STALE_ENTRIES));
    }

    #[test]
    fn a_near_group_gets_a_new_fingerprint_when_a_member_leaves() {
        let (check, cleanup) = after_rewrite(
            "near-leave",
            &near_group("total *= 2;"),
            &functions(&[("first", "total += 1;"), ("second", "total -= 1;")]),
        );
        assert_eq!(check.status.code(), Some(DUPLICATION));
        assert!(check.stdout.contains(NEAR_GROUP));
        assert!(cleanup.contains(STALE_ENTRIES));
    }

    #[test]
    fn an_exact_group_keeps_its_fingerprint_when_a_member_leaves() {
        let (check, cleanup) = after_rewrite(
            "exact-leave",
            &functions(&[("first", ""), ("second", ""), ("third", "")]),
            &functions(&[("first", ""), ("second", "")]),
        );
        assert_eq!(check.status.code(), Some(PASSED));
        assert!(cleanup.contains(NO_STALE_ENTRIES));
    }

    #[test]
    fn a_target_directory_is_pruned_without_a_pattern() {
        let checkout = fixture(
            "pruned",
            &[
                (SOURCE, &lone()),
                (&format!("{PRUNED_DIRECTORY}/generated.rs"), &pair("")),
            ],
        );
        assert_eq!(check(&checkout.dir, NO_NEAR), Ok(()));
    }

    #[test]
    fn a_pattern_matches_a_parent_directory_of_the_checkout() {
        let parent = fixture(
            "parent",
            &[(&format!("{NESTED_CHECKOUT}/{SOURCE}"), &lone())],
        );
        let checkout = parent.dir.join(NESTED_CHECKOUT);
        assert_eq!(check(&checkout, NO_NEAR), Ok(()));
        let args: Vec<&str> = check_args(NO_NEAR)
            .into_iter()
            .chain([EXCLUDE_FLAG, PARENT_PATTERN])
            .collect();
        assert_eq!(tool(&checkout, &args).status.code(), Some(TOOL_ERROR));
    }

    #[test]
    fn cleanup_exits_zero_with_a_stale_entry_and_two_without_sources() {
        let run = tool(&stale("stale").dir, &CLEANUP);
        assert_eq!(run.status.code(), Some(PASSED));
        assert!(run.stdout.contains(STALE_ENTRIES));
        let empty = fixture("empty", &[("README.md", "")]);
        assert_eq!(tool(&empty.dir, &CLEANUP).status.code(), Some(TOOL_ERROR));
    }

    #[test]
    fn the_gate_names_a_stale_entry() {
        assert_eq!(
            no_stale_entry(&stale("gate-stale").dir),
            Err(Rejection::StaleEntry)
        );
        let clean = fixture("gate-clean", &[(SOURCE, &lone())]);
        assert_eq!(no_stale_entry(&clean.dir), Ok(()));
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
}

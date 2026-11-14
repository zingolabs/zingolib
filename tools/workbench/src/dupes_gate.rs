use std::path::Path;

pub const BINARY: &str = "dupes-gate";

const BASELINE: &str = ".dupes-ignore.toml";
const VERSION: &str = "0.2.1";
const MIN_NODES: &str = "10";
const MIN_LINES: &str = "4";
const THRESHOLD: &str = "0.9";
const MAX_EXACT: &str = "0";
const MAX_NEAR: &str = "19";
const EXCLUDE: [&str; 44] = [
    "bin/cli/tests/",
    "crates/netutils/nym-proxy-ffi/tests/",
    "crates/zingolib/src/testutils/",
    "crates/zingolib/src/wallet/disk/testing/",
    "crates/zingolib/tests/",
    "/mocks.rs",
    "/test.rs",
    "/testing.rs",
    "/tests.rs",
    "/testutils.rs",
    "_tests.rs",
    "crates/file-format/src/legacy/corrupt_input.rs",
    "crates/netutils/src/send_response_parsing.rs",
    "crates/pepper-sync/src/shardtree_boundary.rs",
    "crates/pepper-sync/src/sync/truncate/properties.rs",
    "crates/zingolib/src/connectivity/wire_contract.rs",
    "crates/zingolib/src/lightclient/mixnet/proof_acquisition.rs",
    "crates/zingolib/src/lightclient/propose/op_return.rs",
    "crates/zingolib/src/lightclient/propose/pool_matrix.rs",
    "crates/zingolib/src/lightclient/propose/proposal_shape.rs",
    "crates/zingolib/src/lightclient/propose/send_all.rs",
    "crates/zingolib/src/lightclient/propose/shielding.rs",
    "crates/zingolib/src/lightclient/propose/simpool.rs",
    "crates/zingolib/src/lightclient/propose/sync_pause_contract.rs",
    "crates/zingolib/src/lightclient/send/built_transaction_shape.rs",
    "crates/zingolib/src/lightclient/send/op_return.rs",
    "crates/zingolib/src/lightclient/send/transmit_error_seam.rs",
    "crates/zingolib/src/lightclient/send/transparent_policy.rs",
    "crates/zingolib/src/mixnet/driver/wire_contract.rs",
    "crates/zingolib/src/mixnet/mode/demotion.rs",
    "crates/zingolib/src/mixnet/mode/hand_off.rs",
    "crates/zingolib/src/mixnet/mode/stale_proof.rs",
    "crates/zingolib/src/mixnet/mode/wire_contract.rs",
    "crates/zingolib/src/mixnet/pools/bind_failure_absorption.rs",
    "crates/zingolib/src/mixnet/provision/precedence.rs",
    "crates/zingolib/src/wallet/propose/send_all_correction.rs",
    "crates/zingolib/src/wallet/send/proposal_expiry.rs",
    "crates/zingolib/src/wallet/shardtree_boundary.rs",
    "tools/workbench/src/bin/exclusion-audit/compare.rs",
    "tools/workbench/src/bin/exclusion-audit/declared_candidates.rs",
    "tools/workbench/src/bin/exclusion-audit/resolve.rs",
    "tools/workbench/src/bin/test-summary/package_selection_guard.rs",
    "tools/workbench/src/bin/test-summary/parse_failures.rs",
    "tools/workbench/src/bin/test-summary/parse_summary.rs",
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

from_diagnostics!(Rejection::Unrunnable);

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

/// - Runs `cargo dupes --version` as a child process and waits for it.
/// - Unless that is the pinned version, runs `cargo install --force` for the pinned cargo-dupes
///   with empty `RUSTFLAGS`, which overwrites the binary under the cargo home.
fn install() -> Result<(), Rejection> {
    if pinned().is_ok() {
        return Ok(());
    }
    let package = format!("cargo-{SUBCOMMAND}");
    crate::run_streaming(
        crate::CARGO,
        &[
            "install",
            &package,
            "--version",
            VERSION,
            "--locked",
            "--force",
        ],
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
        [flag] if flag == INSTALL_FLAG => install(),
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
mod tests;

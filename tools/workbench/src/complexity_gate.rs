// AGENTS.md states the rule, and this module holds the gate: the
// `complexity-gate` binary pins `rust-code-analysis-cli`, measures the
// cognitive complexity of every function a branch touches before and after,
// and rejects one that grows past the ceiling. A function nobody touched is
// never measured, so pre-existing debt never blocks an unrelated change, and
// a function that only improves is never rejected even if it stays above the
// ceiling.
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub const BINARY: &str = "complexity-gate";

const TOOL: &str = "rust-code-analysis-cli";
const TOOL_VERSION: &str = "0.0.25";
const VERSION_FLAG: &str = "--version";
const INSTALL_FLAG: &str = "--install";
const HELP_FLAG: &str = "--help";
const SHORT_HELP_FLAG: &str = "-h";
const BASE_FLAG: &str = "--base";
const RUSTFLAGS: &str = "RUSTFLAGS";
const NO_FLAGS: &str = "";

/// clippy's own `cognitive_complexity` lint defaults to this threshold. A
/// touched function may reach it, never cross it, and never get worse once
/// already past it.
const CEILING: f64 = 25.0;

const PATH_FLAG: &str = "-p";
const METRICS_FLAG: &str = "-m";
const FORMAT_FLAG: &str = "-O";
const JSON_FORMAT: &str = "json";

const FUNCTION_KIND: &str = "function";
const ANONYMOUS: &str = "<anonymous>";
const RUST_SUFFIX: &str = ".rs";

#[derive(Debug, PartialEq)]
enum Rejection {
    Unrunnable(Vec<String>),
    NotPinned(String),
    ToolFailed(String),
    Regressions(Vec<String>),
}

from_diagnostics!(Rejection::Unrunnable);

impl Rejection {
    fn lines(self) -> Vec<String> {
        match self {
            Rejection::Unrunnable(lines) => lines,
            Rejection::NotPinned(installed) => vec![format!(
                "{installed}, and the gate pins {TOOL} {TOOL_VERSION}; run `{}`",
                gate_command(INSTALL_FLAG)
            )],
            Rejection::ToolFailed(failure) => vec![failure],
            Rejection::Regressions(lines) => lines,
        }
    }
}

fn gate_command(flag: &str) -> String {
    format!("cargo run --manifest-path tools/workbench/Cargo.toml --bin {BINARY} -- {flag}")
}

/// - Runs `rust-code-analysis-cli --version` as a child process and waits for it.
fn installed_version() -> Result<String, Vec<String>> {
    crate::stdout_of(TOOL, &[VERSION_FLAG])
        .map(|version| version.trim().to_string())
        .map_err(|_| {
            vec![
                format!("{TOOL} is not installed"),
                format!("install it with `{}`", gate_command(INSTALL_FLAG)),
            ]
        })
}

fn pinned() -> Result<(), Rejection> {
    let installed = installed_version()?;
    if installed == format!("{TOOL} {TOOL_VERSION}") {
        Ok(())
    } else {
        Err(Rejection::NotPinned(installed))
    }
}

/// - Runs `rust-code-analysis-cli --version` as a child process and waits for it.
/// - Unless that is the pinned version, runs `cargo install --force` for the pinned
///   binary with empty `RUSTFLAGS`, which overwrites it under the cargo home.
fn install() -> Result<(), Rejection> {
    if pinned().is_ok() {
        return Ok(());
    }
    crate::run_streaming(
        crate::CARGO,
        &[
            "install",
            TOOL,
            "--version",
            TOOL_VERSION,
            "--locked",
            "--force",
        ],
        &[(RUSTFLAGS, NO_FLAGS)],
    )?;
    Ok(())
}

/// One `rust-code-analysis-cli` space: a file (`kind: "unit"`), a function, a
/// closure, or a container (`impl`, module, …). Only a named function leaf
/// carries the cognitive-complexity sum this gate compares.
#[derive(serde::Deserialize)]
struct Space {
    name: String,
    kind: String,
    #[serde(default)]
    spaces: Vec<Space>,
    #[serde(default)]
    metrics: Option<Metrics>,
}

#[derive(serde::Deserialize)]
struct Metrics {
    cognitive: Cognitive,
}

#[derive(serde::Deserialize)]
struct Cognitive {
    sum: f64,
}

type Key = (String, String);
type Complexities = BTreeMap<Key, f64>;

/// Collects every named-function leaf under `space` into `out`, keyed by
/// `(file, function name)`. A closure (`name: "<anonymous>"`) is skipped: it
/// has no stable name to match between the before and after measurement.
fn leaves(file: &str, space: &Space, out: &mut Vec<(Key, f64)>) {
    if space.kind == FUNCTION_KIND && space.name != ANONYMOUS {
        if let Some(metrics) = &space.metrics {
            out.push((
                (file.to_string(), space.name.clone()),
                metrics.cognitive.sum,
            ));
        }
    }
    for child in &space.spaces {
        leaves(file, child, out);
    }
}

/// Drops every `(file, function name)` that names more than one function in
/// the same file (overloaded inherent methods across `impl` blocks, for
/// example): the gate has no way to tell which one before matches which one
/// after, so it checks neither rather than guessing.
fn unambiguous(entries: Vec<(Key, f64)>) -> Complexities {
    let mut counts: BTreeMap<Key, usize> = BTreeMap::new();
    for (key, _) in &entries {
        *counts.entry(key.clone()).or_insert(0) += 1;
    }
    entries
        .into_iter()
        .filter(|(key, _)| counts[key] == 1)
        .collect()
}

/// - Runs `rust-code-analysis-cli -p <src> -m -O json` as a child process in `.`, with
///   stderr inherited, and waits for it.
fn measure(src: &Path) -> Result<Complexities, Rejection> {
    let src_arg = src.to_string_lossy().to_string();
    let args = [PATH_FLAG, &src_arg, METRICS_FLAG, FORMAT_FLAG, JSON_FORMAT];
    let finished = crate::finished_in(Path::new("."), TOOL, &args, &[])?;
    if !finished.status.success() {
        return Err(Rejection::ToolFailed(format!(
            "`{TOOL} {}` failed ({})",
            args.join(" "),
            finished.status
        )));
    }
    let mut entries = Vec::new();
    for line in finished.stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let file: Space = serde_json::from_str(line)
            .map_err(|error| Rejection::ToolFailed(format!("{TOOL} output: {error}")))?;
        let relative = Path::new(&file.name)
            .strip_prefix(src)
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or(file.name);
        for child in &file.spaces {
            leaves(&relative, child, &mut entries);
        }
    }
    Ok(unambiguous(entries))
}

/// - Runs `git diff --name-only` in `root` against `merge_base`.
fn changed_rust_files(root: &Path, merge_base: &str) -> Result<Vec<String>, Rejection> {
    let diff = crate::git_in(root, &["diff", "--name-only", merge_base])?;
    Ok(diff
        .lines()
        .map(str::trim)
        .filter(|file| file.ends_with(RUST_SUFFIX))
        .filter(|file| !is_test_path(file))
        .map(str::to_string)
        .collect())
}

/// Whether `path` matches one of [`crate::dupes_gate::EXCLUDE`]'s test-code
/// patterns: test scaffolding carries its own, often deliberately dense,
/// branching and is not the readability debt this gate targets.
fn is_test_path(path: &str) -> bool {
    crate::dupes_gate::EXCLUDE
        .iter()
        .any(|pattern| path.contains(pattern))
}

/// - Runs `git show <merge_base>:<relative>` in `root` for each of `changed`, skipping a
///   file that did not exist at `merge_base`.
/// - Writes each file it finds under `dest` at its own relative path.
fn before_snapshot(
    root: &Path,
    merge_base: &str,
    changed: &[String],
    dest: &Path,
) -> Result<(), Rejection> {
    crate::fresh_dir(dest)?;
    for relative in changed {
        let spec = format!("{merge_base}:{relative}");
        if let Ok(contents) = crate::git_in(root, &["show", &spec]) {
            let file = dest.join(relative);
            crate::create_parent(&file)?;
            crate::write(&file, contents)?;
        }
    }
    Ok(())
}

/// - Reads each of `changed` from the working tree under `root`, skipping a file the
///   branch deleted.
/// - Writes each file it finds under `dest` at its own relative path.
fn after_snapshot(root: &Path, changed: &[String], dest: &Path) -> Result<(), Rejection> {
    crate::fresh_dir(dest)?;
    for relative in changed {
        if let Ok(contents) = crate::read(&root.join(relative)) {
            let file = dest.join(relative);
            crate::create_parent(&file)?;
            crate::write(&file, contents)?;
        }
    }
    Ok(())
}

/// A touched function that grew past the ceiling, or a touched function new
/// to the branch that is already past it. A function that shrank, that held
/// steady, or that stayed under the ceiling never appears here.
fn regressions(before: &Complexities, after: &Complexities) -> Vec<String> {
    let mut lines = Vec::new();
    for (key, &new_value) in after {
        let (file, name) = key;
        match before.get(key) {
            Some(&old_value) if new_value > old_value && new_value > CEILING => {
                lines.push(format!(
                    "{file}:{name} cognitive complexity {old_value:.0} -> {new_value:.0} \
                     (ceiling {CEILING:.0})"
                ));
            }
            None if new_value > CEILING => {
                lines.push(format!(
                    "{file}:{name} is new at cognitive complexity {new_value:.0} \
                     (ceiling {CEILING:.0})"
                ));
            }
            _ => {}
        }
    }
    lines
}

/// A process-wide counter: `cargo test` runs tests as threads of one process, so a
/// temp-directory name keyed only on the process id would collide between concurrent
/// `gate` calls.
static SCRATCH_CALLS: AtomicU64 = AtomicU64::new(0);

fn scratch_name(label: &str) -> String {
    let call = SCRATCH_CALLS.fetch_add(1, Ordering::Relaxed);
    format!("complexity-gate-{label}-{}-{call}", std::process::id())
}

/// - Resolves the merge base against `base`.
/// - Diffs `.rs` files changed since the merge base, skipping test paths.
/// - Measures every touched function's cognitive complexity before (at the merge base)
///   and after (in the working tree).
fn gate(root: &Path, base: &str) -> Result<(), Rejection> {
    pinned()?;
    let merge_base = crate::merge_base(root, base)?;
    let changed = changed_rust_files(root, &merge_base)?;
    if changed.is_empty() {
        return Ok(());
    }
    let before_dir = std::env::temp_dir().join(scratch_name("before"));
    let after_dir = std::env::temp_dir().join(scratch_name("after"));
    before_snapshot(root, &merge_base, &changed, &before_dir)?;
    after_snapshot(root, &changed, &after_dir)?;
    let before_metrics = measure(&before_dir);
    let after_metrics = measure(&after_dir);
    let _ = std::fs::remove_dir_all(&before_dir);
    let _ = std::fs::remove_dir_all(&after_dir);
    let violations = regressions(&before_metrics?, &after_metrics?);
    if violations.is_empty() {
        Ok(())
    } else {
        Err(Rejection::Regressions(violations))
    }
}

/// Prints how to call the gate and what each argument selects.
fn print_usage() {
    println!("usage: {BINARY} [{BASE_FLAG} <ref>] [{INSTALL_FLAG}]");
    println!();
    println!(
        "Measures the cognitive complexity of every function a branch touches, before \
         and after, and rejects one that grows past {CEILING:.0} — clippy's own \
         `cognitive_complexity` default threshold. A function nobody touched is never \
         measured, so existing debt never blocks an unrelated change."
    );
    println!();
    println!(
        "  {BASE_FLAG} <ref>   compare against <ref> instead of {}",
        crate::DEFAULT_BASE
    );
    println!("  {INSTALL_FLAG}      install the pinned {TOOL}");
}

/// - Runs the gate against `root`, or installs the pinned tool when `args` is
///   `--install`, or prints usage when `args` asks for help.
pub fn dispatch(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    if args
        .iter()
        .any(|arg| arg == HELP_FLAG || arg == SHORT_HELP_FLAG)
    {
        print_usage();
        return Ok(());
    }
    if args.iter().any(|arg| arg == INSTALL_FLAG) {
        return install().map_err(Rejection::lines);
    }
    let base = crate::flag_value(args, BASE_FLAG)?
        .unwrap_or(crate::DEFAULT_BASE)
        .to_string();
    gate(root, &base).map_err(Rejection::lines)
}

/// - Reads the process arguments.
/// - Exits the process through [`crate::run`].
pub fn main() -> ! {
    crate::dispatch_from_root(BINARY, dispatch)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "src/lib.rs";
    const GATE_WORKFLOW: &str = ".github/workflows/complexity-delta.yaml";
    const PULL_REQUEST_WORKFLOW: &str = ".github/workflows/ci-pr.yaml";
    const CHECK_NAME: &str = "Complexity delta";

    fn read_root(relative: &str) -> Result<String, Vec<String>> {
        crate::read(&crate::repo_root()?.join(relative))
    }

    /// A function with `depth` nested `if` statements. Measured locally
    /// against the pinned tool: depth 6 sits at cognitive complexity 21
    /// (under the ceiling), depth 7 at 28 (over it).
    fn nested_ifs(name: &str, depth: usize) -> String {
        let mut body = String::from("let mut total = x;\n");
        for level in 0..depth {
            body.push_str(&"    ".repeat(level + 1));
            body.push_str(&format!("if total > {level} {{\n"));
            body.push_str(&"    ".repeat(level + 2));
            body.push_str("total += 1;\n");
        }
        for level in (0..depth).rev() {
            body.push_str(&"    ".repeat(level + 1));
            body.push_str("}\n");
        }
        body.push_str("    total\n");
        format!("pub fn {name}(x: i32) -> i32 {{\n    {body}}}\n")
    }

    const UNDER_CEILING: usize = 6;
    const OVER_CEILING: usize = 7;

    /// Commits `before` as the merge base, then overwrites the working tree with
    /// `after` without committing, so `gate(&dir, &base)` sees exactly the delta a
    /// pull request branch would: `before` at the merge base, `after` live.
    fn delta(
        name: &str,
        before: &str,
        after: &str,
    ) -> Result<(crate::ScratchDir, String), Vec<String>> {
        let dir = crate::scratch_dir(&format!("complexity-gate-{name}"))?;
        crate::create_parent(&dir.join(SOURCE))?;
        crate::write(&dir.join(SOURCE), before)?;
        crate::hermetic_git(&dir, &["init", "-q"])?;
        crate::configure_hermetic_git(&dir)?;
        crate::hermetic_git(&dir, &["add", "."])?;
        crate::hermetic_git(&dir, &["commit", "-q", "-m", "before"])?;
        let base = crate::hermetic_git(&dir, &["rev-parse", "HEAD"])?;
        crate::write(&dir.join(SOURCE), after)?;
        Ok((dir, base))
    }

    #[test]
    fn a_touched_function_that_crosses_the_ceiling_is_rejected() -> Result<(), Vec<String>> {
        let (dir, base) = delta(
            "crosses",
            &nested_ifs("probe", UNDER_CEILING),
            &nested_ifs("probe", OVER_CEILING),
        )?;
        assert!(matches!(gate(&dir, &base), Err(Rejection::Regressions(_))));
        Ok(())
    }

    #[test]
    fn a_touched_function_that_stays_under_the_ceiling_is_not_rejected() -> Result<(), Vec<String>>
    {
        let (dir, base) = delta(
            "under",
            &nested_ifs("probe", 2),
            &nested_ifs("probe", UNDER_CEILING),
        )?;
        assert_eq!(gate(&dir, &base), Ok(()));
        Ok(())
    }

    #[test]
    fn a_new_function_over_the_ceiling_is_rejected() -> Result<(), Vec<String>> {
        let (dir, base) = delta(
            "new-function",
            "pub fn untouched() {}\n",
            &format!(
                "pub fn untouched() {{}}\n\n{}",
                nested_ifs("fresh", OVER_CEILING)
            ),
        )?;
        assert!(matches!(gate(&dir, &base), Err(Rejection::Regressions(_))));
        Ok(())
    }

    #[test]
    fn a_new_function_under_the_ceiling_is_not_rejected() -> Result<(), Vec<String>> {
        let (dir, base) = delta(
            "new-small-function",
            "pub fn untouched() {}\n",
            &format!(
                "pub fn untouched() {{}}\n\n{}",
                nested_ifs("fresh", UNDER_CEILING)
            ),
        )?;
        assert_eq!(gate(&dir, &base), Ok(()));
        Ok(())
    }

    #[test]
    fn a_function_already_over_the_ceiling_that_gets_worse_is_rejected() -> Result<(), Vec<String>>
    {
        let (dir, base) = delta(
            "worse",
            &nested_ifs("probe", OVER_CEILING),
            &nested_ifs("probe", OVER_CEILING + 1),
        )?;
        assert!(matches!(gate(&dir, &base), Err(Rejection::Regressions(_))));
        Ok(())
    }

    #[test]
    fn a_function_already_over_the_ceiling_that_improves_is_not_rejected() -> Result<(), Vec<String>>
    {
        let (dir, base) = delta(
            "improves",
            &nested_ifs("probe", OVER_CEILING + 1),
            &nested_ifs("probe", OVER_CEILING),
        )?;
        assert_eq!(gate(&dir, &base), Ok(()));
        Ok(())
    }

    #[test]
    fn an_unchanged_function_in_a_touched_file_is_not_rejected() -> Result<(), Vec<String>> {
        let before = format!(
            "pub fn touched() {{}}\n\n{}",
            nested_ifs("legacy", OVER_CEILING)
        );
        let after = format!(
            "pub fn touched() {{ let _ = 1; }}\n\n{}",
            nested_ifs("legacy", OVER_CEILING)
        );
        let (dir, base) = delta("untouched-function", &before, &after)?;
        assert_eq!(gate(&dir, &base), Ok(()));
        Ok(())
    }

    #[test]
    fn a_file_the_branch_deletes_is_not_rejected() -> Result<(), Vec<String>> {
        let (dir, base) = delta("deleted", &nested_ifs("probe", OVER_CEILING), "ignored\n")?;
        std::fs::remove_file(dir.join(SOURCE)).map_err(|error| vec![format!("{error}")])?;
        assert_eq!(gate(&dir, &base), Ok(()));
        Ok(())
    }

    #[test]
    fn an_ambiguous_duplicate_name_is_never_compared() -> Result<(), Vec<String>> {
        let before = format!(
            "{}\nmod other {{\n    {}\n}}\n",
            nested_ifs("probe", UNDER_CEILING),
            nested_ifs("probe", UNDER_CEILING)
        );
        let after = format!(
            "{}\nmod other {{\n    {}\n}}\n",
            nested_ifs("probe", OVER_CEILING),
            nested_ifs("probe", OVER_CEILING)
        );
        let (dir, base) = delta("ambiguous", &before, &after)?;
        assert_eq!(gate(&dir, &base), Ok(()));
        Ok(())
    }

    #[test]
    fn a_non_rust_change_is_not_rejected() -> Result<(), Vec<String>> {
        let (dir, base) = delta(
            "no-rust-change",
            "pub fn probe() {}\n",
            "pub fn probe() {}\n",
        )?;
        crate::write(&dir.join("README.md"), "before\n")?;
        crate::hermetic_git(&dir, &["add", "README.md"])?;
        crate::hermetic_git(&dir, &["commit", "-q", "-m", "add readme"])?;
        crate::write(&dir.join("README.md"), "after\n")?;
        assert_eq!(gate(&dir, &base), Ok(()));
        Ok(())
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
        assert!(!workflow.contains(BINARY));
        assert!(!workflow.contains(CHECK_NAME));
        Ok(())
    }

    #[test]
    fn the_gate_passes_on_this_checkout() -> Result<(), Vec<String>> {
        assert_eq!(gate(&crate::repo_root()?, crate::DEFAULT_BASE), Ok(()));
        Ok(())
    }
}

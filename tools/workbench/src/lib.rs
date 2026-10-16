//! Shared helpers for the workbench tooling crate (one binary per `src/bin/*.rs`).
//!
//! Every tool follows the same shape: resolve something under the repo root,
//! then either print a result or emit one-or-more `"{prog}: {line}"`
//! diagnostics and exit non-zero. [`run`] centralises that `main()` shape, and
//! [`repo_root`], [`git`], and [`toolchain_channel`] are the shared primitives.

#![forbid(unsafe_code)]

macro_rules! from_diagnostics {
    ($error:ident :: $variant:ident) => {
        impl From<Vec<String>> for $error {
            fn from(diagnostics: Vec<String>) -> Self {
                $error::$variant(diagnostics)
            }
        }
    };
}

pub mod binding_changelog;
pub mod binding_layer;
pub mod binding_manifest;
pub mod binding_publish;
pub mod closed_pr_runs;
pub mod dupes_gate;
pub mod nextest_ceilings;
pub mod orasust;
pub mod panic_strategy;
pub mod runner_labels;
pub mod session;
pub mod test_placement;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{exit, Command, ExitStatus, Stdio};

/// Run a tool `body`, reporting diagnostics as `"{prog}: {line}"` to stderr and
/// exiting `1` on error. On success runs `on_ok` (e.g. to print a result) and
/// exits `0`. This is the single `main()` shape shared by every binary.
pub fn run<T>(
    prog: &str,
    body: impl FnOnce() -> Result<T, Vec<String>>,
    on_ok: impl FnOnce(T),
) -> ! {
    match body() {
        Ok(value) => {
            on_ok(value);
            exit(0);
        }
        Err(lines) => {
            for line in lines {
                eprintln!("{prog}: {line}");
            }
            exit(1);
        }
    }
}

/// - Reads the process arguments.
/// - Exits the process through [`run`].
pub fn dispatch_from_root(
    binary: &str,
    dispatch: fn(&Path, &[String]) -> Result<(), Vec<String>>,
) -> ! {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run(binary, || dispatch(&repo_root()?, &args), |()| ())
}

/// Run `<program> <args>` with stderr inherited and return its stdout, or a one-line diagnostic on failure.
pub fn stdout_of(program: &str, args: &[&str]) -> Result<String, Vec<String>> {
    stdout_with_env(program, args, &[])
}

/// Run `<program> <args>` with extra environment, exactly as [`stdout_of`] does otherwise.
pub fn stdout_with_env(
    program: &str,
    args: &[&str],
    env: &[(&str, &str)],
) -> Result<String, Vec<String>> {
    stdout_in(Path::new(CURRENT_DIR), program, args, env)
}

/// The working directory that a command inherits unless a caller names another.
const CURRENT_DIR: &str = ".";

/// Run `<program> <args>` from a directory with extra environment, exactly as [`stdout_of`] does otherwise.
pub fn stdout_in(
    directory: &Path,
    program: &str,
    args: &[&str],
    env: &[(&str, &str)],
) -> Result<String, Vec<String>> {
    let finished = finished_in(directory, program, args, env)?;
    if !finished.status.success() {
        return Err(vec![format!("`{program} {}` failed", args.join(" "))]);
    }
    Ok(finished.stdout)
}

pub struct Finished {
    pub status: ExitStatus,
    pub stdout: String,
}

/// - Runs `<program> <args>` as a child process in `directory`, with stderr inherited, and waits for it.
pub fn finished_in(
    directory: &Path,
    program: &str,
    args: &[&str],
    env: &[(&str, &str)],
) -> Result<Finished, Vec<String>> {
    let output = Command::new(program)
        .args(args)
        .current_dir(directory)
        .envs(env.iter().copied())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| vec![format!("failed to run {program}: {e}")])?;
    let stdout = String::from_utf8(output.stdout)
        .map_err(|e| vec![format!("{program} output not utf-8: {e}")])?;
    Ok(Finished {
        status: output.status,
        stdout,
    })
}

pub const CARGO: &str = "cargo";
const VERSION_FLAG: &str = "--version";

/// - Runs `cargo <name> --version` as a child process, with stderr inherited, and waits for it.
pub fn cargo_subcommand_version(name: &str, install_command: &str) -> Result<String, Vec<String>> {
    stdout_of(CARGO, &[name, VERSION_FLAG])
        .map(|version| version.trim().to_string())
        .map_err(|_| {
            vec![
                format!("cargo-{name} is not installed"),
                format!("install it with `{install_command}`"),
            ]
        })
}

/// Run `<program> <args>` with extra environment and all output streamed, and fail if it fails.
pub fn run_streaming(
    program: &str,
    args: &[&str],
    env: &[(&str, &str)],
) -> Result<(), Vec<String>> {
    let status = Command::new(program)
        .args(args)
        .envs(env.iter().copied())
        .status()
        .map_err(|e| vec![format!("failed to run {program}: {e}")])?;
    if status.success() {
        Ok(())
    } else {
        Err(vec![format!(
            "`{program} {}` failed ({status})",
            args.join(" ")
        )])
    }
}

/// Run `<program> <args>` over owned arguments, exactly as [`stdout_of`] does.
pub fn stdout_of_owned(program: &str, args: &[String]) -> Result<String, Vec<String>> {
    stdout_of(
        program,
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    )
}

/// A path as UTF-8, or a one-line diagnostic naming it.
pub fn utf8(file: &Path) -> Result<&str, Vec<String>> {
    file.to_str()
        .ok_or_else(|| vec![format!("{} is not valid UTF-8", file.display())])
}

/// Create the parent directory of a file, and any missing ancestors.
pub fn create_parent(file: &Path) -> Result<(), Vec<String>> {
    file.parent().map_or(Ok(()), |parent| {
        std::fs::create_dir_all(parent)
            .map_err(|e| vec![format!("cannot create {}: {e}", parent.display())])
    })
}

/// Remove a directory if it exists, create it empty, and return its path.
pub fn fresh_dir(directory: &Path) -> Result<PathBuf, Vec<String>> {
    if directory.exists() {
        std::fs::remove_dir_all(directory)
            .map_err(|e| vec![format!("cannot clear {}: {e}", directory.display())])?;
    }
    std::fs::create_dir_all(directory)
        .map_err(|e| vec![format!("cannot create {}: {e}", directory.display())])?;
    Ok(directory.to_path_buf())
}

const GIT: &str = "git";

/// Run `git <args>` and return its stdout, or a one-line diagnostic on failure.
pub fn git(args: &[&str]) -> Result<String, Vec<String>> {
    git_in(Path::new(CURRENT_DIR), args)
}

/// - Runs `git <args>` as a child process in `directory`, with stderr inherited, and waits for it.
pub fn git_in(directory: &Path, args: &[&str]) -> Result<String, Vec<String>> {
    stdout_in(directory, GIT, args, &[])
}

#[cfg(test)]
const HERMETIC_GIT_CONFIG: [(&str, &str); 3] = [
    ("user.name", "workbench"),
    ("user.email", "workbench@example.invalid"),
    ("commit.gpgsign", "false"),
];

#[cfg(test)]
const HERMETIC_GIT_ENV: [(&str, &str); 2] = [
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
];

#[cfg(test)]
const CONFIG_FLAG: &str = "-c";

/// - Runs `git <args>` as a child process in `directory` under [`HERMETIC_GIT_CONFIG`],
///   with the global and system git config masked by [`HERMETIC_GIT_ENV`].
#[cfg(test)]
pub(crate) fn hermetic_git(directory: &Path, args: &[&str]) -> Result<String, Vec<String>> {
    let settings: Vec<String> = HERMETIC_GIT_CONFIG
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    let mut full: Vec<&str> = settings
        .iter()
        .flat_map(|setting| [CONFIG_FLAG, setting.as_str()])
        .collect();
    full.extend_from_slice(args);
    Ok(stdout_in(directory, GIT, &full, &HERMETIC_GIT_ENV)?
        .trim()
        .to_string())
}

/// - Runs `git config` in `directory` for each setting of [`HERMETIC_GIT_CONFIG`].
#[cfg(test)]
pub(crate) fn configure_hermetic_git(directory: &Path) -> Result<(), Vec<String>> {
    for (key, value) in HERMETIC_GIT_CONFIG {
        hermetic_git(directory, &["config", key, value])?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) struct ScratchDir(PathBuf);

#[cfg(test)]
impl std::ops::Deref for ScratchDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

/// - Removes the directory and everything under it.
#[cfg(test)]
impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// - Removes and recreates `workbench-<name>-<pid>` under the system temp dir.
#[cfg(test)]
pub(crate) fn scratch_dir(name: &str) -> Result<ScratchDir, Vec<String>> {
    let dir = std::env::temp_dir().join(format!("workbench-{name}-{}", std::process::id()));
    Ok(ScratchDir(fresh_dir(&dir)?))
}

pub fn required<T>(value: Option<T>, what: &str) -> Result<T, Vec<String>> {
    value.ok_or_else(|| vec![format!("no {what}")])
}

pub fn refused<T>(result: Result<T, Vec<String>>) -> Result<Vec<String>, Vec<String>> {
    match result {
        Err(diagnostics) => Ok(diagnostics),
        Ok(_) => Err(vec!["expected a refusal, got a success".to_string()]),
    }
}

/// - Runs `git ls-tree` in `root`.
pub fn listed_at(root: &Path, revision: &str, relative: &str) -> Result<bool, Vec<String>> {
    git_in(root, &["ls-tree", "--name-only", revision, "--", relative])
        .map(|listed| !listed.trim().is_empty())
}

pub fn commit_spec(revision: &str) -> String {
    format!("{revision}^{{commit}}")
}

/// - Runs `git rev-parse` in `root`.
pub fn commit_of(root: &Path, revision: &str) -> Result<String, Vec<String>> {
    git_in(
        root,
        &["rev-parse", "--verify", "--quiet", &commit_spec(revision)],
    )
    .map(|sha| sha.trim().to_string())
    .map_err(|_| vec![format!("{revision} is not a commit of this repository")])
}

/// - Runs `git merge-base --is-ancestor` in `root`.
pub fn is_ancestor(root: &Path, ancestor: &str, descendant: &str) -> Result<bool, Vec<String>> {
    let args = ["merge-base", "--is-ancestor", ancestor, descendant];
    let finished = finished_in(root, GIT, &args, &[])?;
    match finished.status.code() {
        Some(IS_ANCESTOR) => Ok(true),
        Some(IS_NOT_ANCESTOR) => Ok(false),
        _ => Err(vec![format!("`{GIT} {}` failed", args.join(" "))]),
    }
}

const IS_ANCESTOR: i32 = 0;
const IS_NOT_ANCESTOR: i32 = 1;

pub fn file_name_of(file: &Path) -> Result<&str, Vec<String>> {
    file.file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| vec![format!("{} has no file name", file.display())])
}

pub const BRANCH_TIP: &str = "HEAD";

pub const DEFAULT_BASE: &str = "origin/dev";

pub const FALLBACK_BASE: &str = "dev";

/// - Runs `git merge-base` in `root`, again against the fallback when the default base is absent.
pub fn merge_base(root: &Path, base: &str) -> Result<String, Vec<String>> {
    let found = |reference: &str| {
        git_in(root, &["merge-base", BRANCH_TIP, reference]).map(|commit| commit.trim().to_string())
    };
    found(base)
        .or_else(|absent| {
            if base == DEFAULT_BASE {
                found(FALLBACK_BASE)
            } else {
                Err(absent)
            }
        })
        .map_err(|_| {
            vec![
                format!("no merge base between HEAD and '{base}'"),
                "name a base that exists with --base <ref>".to_string(),
            ]
        })
}

const ACTIONS_ENV: &str = "GITHUB_ACTIONS";
const ACTIONS_WARNING: &str = "::warning::";
const WARNING_PREFIX: &str = "warning: ";

pub fn warning_line(message: &str, in_actions: bool) -> String {
    let prefix = if in_actions {
        ACTIONS_WARNING
    } else {
        WARNING_PREFIX
    };
    format!("{prefix}{message}")
}

/// - Reads `GITHUB_ACTIONS` from the environment.
/// - Writes one line to stderr.
pub fn warn(message: &str) {
    let in_actions = std::env::var_os(ACTIONS_ENV).is_some();
    eprintln!("{}", warning_line(message, in_actions));
}

const LOCKFILE: &str = "Cargo.lock";
pub const MANIFEST_PATH_FLAG: &str = "--manifest-path";
const LOCKED_FLAG: &str = "--locked";

/// - Runs `cargo metadata` as a child process, resolving dependencies only when `deps` is set.
pub fn metadata_of(manifest: &Path, deps: bool) -> Result<cargo_metadata::Metadata, Vec<String>> {
    let mut command = cargo_metadata::MetadataCommand::new();
    command.manifest_path(manifest);
    if deps {
        command.other_options([LOCKED_FLAG.to_string()]);
    } else {
        command.no_deps();
    }
    command.exec().map_err(|e| {
        vec![format!(
            "`{CARGO} metadata` failed for {}: {e}",
            manifest.display()
        )]
    })
}

/// - Resolves `path` through the filesystem, following symbolic links.
pub fn canonical(path: &Path) -> Result<PathBuf, Vec<String>> {
    at_path(path, "resolve", |path| std::fs::canonicalize(path))
}

struct ManifestLookup {
    by_directory: BTreeMap<PathBuf, (PathBuf, bool)>,
}

impl ManifestLookup {
    /// - Runs `git ls-files` in `root` through [`workspace_manifests`].
    /// - Runs `cargo metadata` through [`metadata_of`] once per workspace.
    fn of(root: &Path) -> Result<Self, Vec<String>> {
        let mut by_directory = BTreeMap::new();
        for workspace in workspace_manifests(root)? {
            let metadata = metadata_of(&canonical(&workspace)?, false)?;
            by_directory
                .entry(metadata.workspace_root.clone().into_std_path_buf())
                .or_insert_with(|| {
                    (
                        metadata.workspace_root.join(MANIFEST).into_std_path_buf(),
                        false,
                    )
                });
            for package in &metadata.packages {
                if let Some(crate_dir) = package.manifest_path.parent() {
                    by_directory.insert(
                        crate_dir.as_std_path().to_path_buf(),
                        (package.manifest_path.clone().into_std_path_buf(), true),
                    );
                }
            }
        }
        Ok(Self { by_directory })
    }

    /// - Resolves `dir` through the filesystem, following symbolic links.
    fn nearest(&self, dir: &Path) -> Result<(PathBuf, bool), Vec<String>> {
        let dir = canonical(dir)?;
        dir.ancestors()
            .find_map(|ancestor| self.by_directory.get(ancestor).cloned())
            .ok_or_else(|| vec![format!("no workspace holds {}", dir.display())])
    }
}

/// - Runs `cargo metadata` through [`metadata_of`].
pub fn declares_a_package(manifest: &Path) -> Result<bool, Vec<String>> {
    let manifest = canonical(manifest)?;
    Ok(metadata_of(&manifest, false)?
        .packages
        .iter()
        .any(|package| package.manifest_path.as_std_path() == manifest))
}

/// - Runs `cargo metadata` through [`metadata_of`].
pub fn workspace_members(manifest: &Path) -> Result<Vec<PathBuf>, Vec<String>> {
    Ok(metadata_of(manifest, false)?
        .workspace_packages()
        .iter()
        .map(|package| package.manifest_path.clone().into_std_path_buf())
        .collect())
}

/// - Runs `git ls-files` in `root`.
pub fn workspace_manifests(root: &Path) -> Result<Vec<PathBuf>, Vec<String>> {
    let nested = format!("*/{LOCKFILE}");
    let lockfiles = git_in(root, &["ls-files", "--", LOCKFILE, &nested])?;
    Ok(lockfiles
        .lines()
        .map(|lockfile| root.join(lockfile).with_file_name(MANIFEST))
        .collect())
}

/// - Runs `git diff` and `git ls-files` in `root`, then `cargo metadata` once per workspace.
pub fn touched_manifests(root: &Path, merge_base: &str) -> Result<Vec<PathBuf>, Vec<String>> {
    Ok(touched(root, merge_base)?.into_keys().collect())
}

/// - Runs the same child processes as [`touched_manifests`].
pub fn touched_packages(root: &Path, merge_base: &str) -> Result<Vec<PathBuf>, Vec<String>> {
    Ok(touched(root, merge_base)?
        .into_iter()
        .filter_map(|(manifest, declares_a_package)| declares_a_package.then_some(manifest))
        .collect())
}

fn touched(root: &Path, merge_base: &str) -> Result<BTreeMap<PathBuf, bool>, Vec<String>> {
    let changed = git_in(root, &["diff", "--name-only", merge_base])?;
    let dirs: BTreeSet<PathBuf> = changed
        .lines()
        .map(|file| existing_dir_of(root, Path::new(file)))
        .collect();
    let lookup = ManifestLookup::of(root)?;
    let mut manifests = BTreeMap::new();
    for dir in dirs {
        let (manifest, declares) = lookup.nearest(&dir)?;
        if manifests.contains_key(&manifest) {
            continue;
        }
        let beside = manifest.parent() == Some(dir.as_path());
        if beside || declares {
            manifests.insert(manifest, declares);
        }
    }
    Ok(manifests)
}

fn existing_dir_of(root: &Path, file: &Path) -> PathBuf {
    root.join(file)
        .ancestors()
        .skip(1)
        .find(|dir| dir.is_dir())
        .map_or_else(|| root.to_path_buf(), Path::to_path_buf)
}

pub fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

/// The workbench crate's directory at the time cargo compiled the crate.
const BUILT_WORKBENCH_DIR: &str = env!("CARGO_MANIFEST_DIR");

/// The workbench crate's directory, relative to the zingolib root.
const WORKBENCH_RELATIVE_DIR: &str = "tools/workbench";

/// The manifest file name that every crate directory holds.
pub const MANIFEST: &str = "Cargo.toml";

pub const LIST_SEPARATOR: &str = ", ";

/// The zingolib root, which is the directory that holds the workbench crate at `tools/workbench`.
/// - Reads every directory under `root`, skipping any directory named in `skipped_directories`
///   and never following symbolic links.
pub fn files_under(root: &Path, skipped_directories: &[&str]) -> Result<Vec<PathBuf>, Vec<String>> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let unreadable = |error: std::io::Error| vec![format!("{}: {error}", directory.display())];
        for entry in std::fs::read_dir(&directory).map_err(unreadable)? {
            let entry = entry.map_err(unreadable)?;
            let file_type = entry.file_type().map_err(unreadable)?;
            let path = entry.path();
            if file_type.is_dir() {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default();
                if !skipped_directories.contains(&name) {
                    pending.push(path);
                }
            } else if file_type.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

pub fn repo_root() -> Result<PathBuf, Vec<String>> {
    root_above(Path::new(BUILT_WORKBENCH_DIR))
}

/// The zingolib root above a workbench crate directory, or a diagnostic that names the directory.
fn root_above(workbench_dir: &Path) -> Result<PathBuf, Vec<String>> {
    let depth = Path::new(WORKBENCH_RELATIVE_DIR).components().count();
    Some(workbench_dir)
        .filter(|dir| dir.ends_with(WORKBENCH_RELATIVE_DIR) && dir.join(MANIFEST).is_file())
        .and_then(|dir| dir.ancestors().nth(depth))
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            vec![format!(
                "{} is not a workbench crate directory at <zingolib>/{WORKBENCH_RELATIVE_DIR}",
                workbench_dir.display()
            )]
        })
}

pub fn verdict(diagnostics: Vec<String>) -> Result<(), Vec<String>> {
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

fn at_path<T>(
    path: &Path,
    verb: &str,
    action: impl FnOnce(&Path) -> std::io::Result<T>,
) -> Result<T, Vec<String>> {
    action(path).map_err(|e| vec![format!("cannot {verb} {}: {e}", path.display())])
}

/// - Writes `contents` to `path`.
pub fn write(path: &Path, contents: impl AsRef<[u8]>) -> Result<(), Vec<String>> {
    at_path(path, "write", |path| std::fs::write(path, contents))
}

pub fn read(path: &Path) -> Result<String, Vec<String>> {
    let bytes = at_path(path, "read", |path| std::fs::read(path))?;
    String::from_utf8(bytes).map_err(|e| vec![format!("{} is not UTF-8: {e}", path.display())])
}

/// The value of the first `<flag> <value>` or `<flag>=<value>` argument, if present.
pub fn flag_value<'a>(args: &'a [String], flag: &str) -> Result<Option<&'a str>, Vec<String>> {
    flag_value_after(args, flag, &format!("{flag}="))
}

/// The recursive step of [`flag_value`], given the flag's joined `<flag>=` prefix.
fn flag_value_after<'a>(
    args: &'a [String],
    flag: &str,
    joined_prefix: &str,
) -> Result<Option<&'a str>, Vec<String>> {
    match args {
        [] => Ok(None),
        [arg, rest @ ..] => match arg.strip_prefix(joined_prefix) {
            Some(value) => Ok(Some(value)),
            None if arg == flag => rest
                .first()
                .map(|value| Some(value.as_str()))
                .ok_or_else(|| vec![format!("{flag} requires a value")]),
            None => flag_value_after(rest, flag, joined_prefix),
        },
    }
}

pub fn required_flag<'a>(
    args: &'a [String],
    flag: &str,
    usage: &str,
) -> Result<&'a str, Vec<String>> {
    flag_value(args, flag)?.ok_or_else(|| vec![format!("missing {flag}"), usage.to_string()])
}

/// The value of a `--dest <dir>` or `--dest=<dir>` argument, if present.
pub fn parse_dest(args: &[String]) -> Result<Option<PathBuf>, Vec<String>> {
    Ok(flag_value(args, "--dest")?.map(PathBuf::from))
}

pub const TOOLCHAIN_FILE: &str = "rust-toolchain.toml";

/// The pinned, validated rustc channel from `<root>/rust-toolchain.toml`.
///
/// Single source of truth for `RUST_VERSION`. Rejects any non-numeric channel
/// (`stable` / `nightly` / dated pins) so the CI image tag stays reproducible.
pub fn toolchain_channel(root: &Path) -> Result<String, Vec<String>> {
    let path = root.join(TOOLCHAIN_FILE);
    let contents = read(&path)?;

    let Some(channel) = contents.lines().find_map(channel_value) else {
        return Err(vec![format!(
            "no [toolchain].channel in {}",
            path.display()
        )]);
    };

    if !is_concrete_numeric(&channel) {
        return Err(vec![
            format!("channel '{channel}' is not a concrete numeric version (e.g. 1.91 or 1.91.0)"),
            format!(
                "a pinned rustc is required; set channel = \"<x.y[.z]>\" in {}",
                path.display()
            ),
        ]);
    }
    Ok(channel)
}

/// Value of a `channel = "..."` line, mirroring `^[[:space:]]*channel[[:space:]]*=`.
/// `None` for comments, other keys, or a line without a double-quoted value.
fn channel_value(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("channel")?.trim_start();
    let value = rest.strip_prefix('=')?.trim_start().strip_prefix('"')?;
    let end = value.find('"')?;
    Some(value[..end].to_string())
}

/// `^[0-9]+\.[0-9]+(\.[0-9]+)?$`, two or three dot-separated all-digit parts.
fn is_concrete_numeric(channel: &str) -> bool {
    let parts: Vec<&str> = channel.split('.').collect();
    matches!(parts.len(), 2 | 3)
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn files_under_walks_every_directory_but_the_skipped_ones_in_sorted_order(
    ) -> Result<(), Vec<String>> {
        let root = std::env::temp_dir().join(format!(
            "workbench-files-under-{}-{}",
            std::process::id(),
            line!()
        ));
        let io = |error: std::io::Error| vec![format!("{}: {error}", root.display())];
        std::fs::create_dir_all(root.join("src/deep")).map_err(io)?;
        std::fs::create_dir_all(root.join("target/debug")).map_err(io)?;
        std::fs::write(root.join("src/deep/b.rs"), "").map_err(io)?;
        std::fs::write(root.join("src/a.rs"), "").map_err(io)?;
        std::fs::write(root.join("README.md"), "").map_err(io)?;
        std::fs::write(root.join("target/debug/generated.rs"), "").map_err(io)?;
        let files = super::files_under(&root, &["target"])?;
        std::fs::remove_dir_all(&root).map_err(io)?;
        assert_eq!(
            files,
            [
                root.join("README.md"),
                root.join("src/a.rs"),
                root.join("src/deep/b.rs"),
            ]
        );
        assert!(
            super::files_under(&root, &[]).is_err(),
            "a missing root is an error"
        );
        Ok(())
    }

    #[test]
    fn a_warning_is_a_workflow_command_only_under_actions() {
        assert_eq!(
            super::warning_line("base is gone", true),
            "::warning::base is gone"
        );
        assert_eq!(
            super::warning_line("base is gone", false),
            "warning: base is gone"
        );
    }

    use super::*;

    /// A workbench crate directory under a checkout that was moved or removed after the build.
    const ABSENT_WORKBENCH_DIR: &str = "/absent/zingolib/tools/workbench";

    /// - Runs the same child processes as [`ManifestLookup::of`] over the repo root.
    fn manifest_above(dir: &Path) -> Result<PathBuf, Vec<String>> {
        ManifestLookup::of(&repo_root()?)?
            .nearest(dir)
            .map(|(manifest, _)| manifest)
    }

    #[test]
    fn the_root_is_two_directories_above_the_workbench_crate() -> Result<(), Vec<String>> {
        let workbench_dir = Path::new(BUILT_WORKBENCH_DIR);
        let tools_dir = required(workbench_dir.parent(), "parent of the workbench directory")?;
        assert_eq!(
            root_above(workbench_dir)?,
            required(tools_dir.parent(), "parent of the tools directory")?
        );
        Ok(())
    }

    #[test]
    fn repo_root_is_the_root_above_the_built_crate_directory() -> Result<(), Vec<String>> {
        assert_eq!(repo_root()?, root_above(Path::new(BUILT_WORKBENCH_DIR))?);
        Ok(())
    }

    #[test]
    fn a_foreign_manifest_directory_in_the_environment_does_not_move_the_root(
    ) -> Result<(), Vec<String>> {
        let test_binary = std::env::current_exe()
            .map_err(|e| vec![format!("cannot locate the test binary: {e}")])?;
        let inner = Command::new(test_binary)
            .env("CARGO_MANIFEST_DIR", ABSENT_WORKBENCH_DIR)
            .args([
                "--exact",
                "tests::repo_root_is_the_root_above_the_built_crate_directory",
            ])
            .stdout(Stdio::null())
            .status()
            .map_err(|e| vec![format!("cannot rerun the test binary: {e}")])?;
        assert!(inner.success());
        Ok(())
    }

    #[test]
    fn a_directory_that_is_not_the_workbench_crate_is_refused_by_name() -> Result<(), Vec<String>> {
        let source_dir = Path::new(BUILT_WORKBENCH_DIR).join("src");
        let diagnostic = refused(root_above(&source_dir))?.concat();
        assert!(diagnostic.contains(utf8(&source_dir)?));
        Ok(())
    }

    #[test]
    fn a_workbench_crate_directory_that_is_absent_is_refused_by_name() -> Result<(), Vec<String>> {
        let diagnostic = refused(root_above(Path::new(ABSENT_WORKBENCH_DIR)))?.concat();
        assert!(diagnostic.contains(ABSENT_WORKBENCH_DIR));
        Ok(())
    }

    #[test]
    fn channel_value_recognises_only_quoted_assignments() {
        assert_eq!(
            channel_value("channel = \"1.91.0\"").as_deref(),
            Some("1.91.0")
        );
        assert_eq!(channel_value("  channel=\"1.91\"").as_deref(), Some("1.91"));
        assert_eq!(channel_value("# channel = \"x\""), None);
        assert_eq!(channel_value("components = [\"clippy\"]"), None);
        assert_eq!(channel_value("[toolchain]"), None);
    }

    #[test]
    fn parses_separate_and_joined_dest() -> Result<(), Vec<String>> {
        let sep = vec![
            "--release".to_string(),
            "--dest".to_string(),
            "/x".to_string(),
        ];
        assert_eq!(parse_dest(&sep)?, Some(PathBuf::from("/x")));

        let joined = vec!["--dest=/y".to_string()];
        assert_eq!(parse_dest(&joined)?, Some(PathBuf::from("/y")));
        Ok(())
    }

    #[test]
    fn no_dest_is_none() -> Result<(), Vec<String>> {
        assert_eq!(parse_dest(&["--release".to_string()])?, None);
        Ok(())
    }

    #[test]
    fn dest_without_value_is_an_error() {
        assert!(parse_dest(&["--dest".to_string()]).is_err());
    }

    #[test]
    fn the_manifest_above_a_nested_member_source_is_that_member() -> Result<(), Vec<String>> {
        let root = repo_root()?;
        assert_eq!(
            manifest_above(&root.join("zingo-ffi/lib/src"))?,
            root.join("zingo-ffi/lib").join(MANIFEST)
        );
        assert_eq!(manifest_above(&root.join("docs"))?, root.join(MANIFEST));
        Ok(())
    }

    #[test]
    fn a_virtual_manifest_declares_no_package() -> Result<(), Vec<String>> {
        let root = repo_root()?;
        assert!(!declares_a_package(&root.join(MANIFEST))?);
        assert!(declares_a_package(&root.join("zingo-cli").join(MANIFEST))?);
        Ok(())
    }

    #[test]
    fn the_root_workspace_members_include_the_nested_ones() -> Result<(), Vec<String>> {
        let root = repo_root()?;
        let members = workspace_members(&root.join(MANIFEST))?;
        for member in ["zingo-ffi/lib", "zingo-ffi/uniffi-bindgen", "zingo-cli"] {
            assert!(
                members.contains(&root.join(member).join(MANIFEST)),
                "{member}"
            );
        }
        Ok(())
    }

    #[test]
    fn every_workspace_is_found_by_its_lockfile() -> Result<(), Vec<String>> {
        let root = repo_root()?;
        let found = workspace_manifests(&root)?;
        for workspace in ["", "zingo-netutils", WORKBENCH_RELATIVE_DIR] {
            assert!(
                found.contains(&root.join(workspace).join(MANIFEST)),
                "{workspace:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn a_deleted_file_resolves_through_its_nearest_existing_directory() -> Result<(), Vec<String>> {
        let root = repo_root()?;
        assert_eq!(
            existing_dir_of(&root, Path::new("zingo-cli/src/gone/file.rs")),
            root.join("zingo-cli/src")
        );
        assert_eq!(existing_dir_of(&root, Path::new("README.md")), root);
        Ok(())
    }

    #[test]
    fn numeric_validation_matches_x_y_z() {
        assert!(is_concrete_numeric("1.91.0"));
        assert!(is_concrete_numeric("1.91"));
        assert!(!is_concrete_numeric("stable"));
        assert!(!is_concrete_numeric("nightly"));
        assert!(!is_concrete_numeric("1"));
        assert!(!is_concrete_numeric("1.91.0.1"));
        assert!(!is_concrete_numeric("1..0"));
    }
}

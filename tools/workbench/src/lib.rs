//! Shared helpers for the workbench tooling crate (one binary per `src/bin/*.rs`).
//!
//! Every tool follows the same shape: resolve something under the repo root,
//! then either print a result or emit one-or-more `"{prog}: {line}"`
//! diagnostics and exit non-zero. [`run`] centralises that `main()` shape, and
//! [`repo_root`], [`git`], and [`toolchain_channel`] are the shared primitives.

#![forbid(unsafe_code)]

pub mod binding_changelog;
pub mod binding_layer;
pub mod dupes_gate;

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

/// - Reads the process arguments, runs `dispatch` against the zingolib root, and exits through [`run`].
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

/// Run `git <args>` and return its stdout, or a one-line diagnostic on failure.
pub fn git(args: &[&str]) -> Result<String, Vec<String>> {
    stdout_of("git", args)
}

/// The workbench crate's directory at the time cargo compiled the crate.
const BUILT_WORKBENCH_DIR: &str = env!("CARGO_MANIFEST_DIR");

/// The workbench crate's directory, relative to the zingolib root.
const WORKBENCH_RELATIVE_DIR: &str = "tools/workbench";

/// The manifest file name that every crate directory holds.
pub const MANIFEST: &str = "Cargo.toml";

/// The zingolib root, which is the directory that holds the workbench crate at `tools/workbench`.
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

/// Read `path` to a string, or a one-line `cannot read …` diagnostic.
pub fn read(path: &Path) -> Result<String, Vec<String>> {
    std::fs::read_to_string(path).map_err(|e| vec![format!("cannot read {}: {e}", path.display())])
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
    use super::*;

    /// A workbench crate directory under a checkout that was moved or removed after the build.
    const ABSENT_WORKBENCH_DIR: &str = "/absent/zingolib/tools/workbench";

    #[test]
    fn the_root_is_two_directories_above_the_workbench_crate() {
        let workbench_dir = Path::new(BUILT_WORKBENCH_DIR);
        assert_eq!(
            root_above(workbench_dir).unwrap(),
            workbench_dir.parent().unwrap().parent().unwrap()
        );
    }

    #[test]
    fn repo_root_is_the_root_above_the_built_crate_directory() {
        assert_eq!(
            repo_root().unwrap(),
            root_above(Path::new(BUILT_WORKBENCH_DIR)).unwrap()
        );
    }

    #[test]
    fn a_foreign_manifest_directory_in_the_environment_does_not_move_the_root() {
        let inner = Command::new(std::env::current_exe().unwrap())
            .env("CARGO_MANIFEST_DIR", ABSENT_WORKBENCH_DIR)
            .args([
                "--exact",
                "tests::repo_root_is_the_root_above_the_built_crate_directory",
            ])
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(inner.success());
    }

    #[test]
    fn a_directory_that_is_not_the_workbench_crate_is_refused_by_name() {
        let source_dir = Path::new(BUILT_WORKBENCH_DIR).join("src");
        let diagnostic = root_above(&source_dir).unwrap_err().concat();
        assert!(diagnostic.contains(source_dir.to_str().unwrap()));
    }

    #[test]
    fn a_workbench_crate_directory_that_is_absent_is_refused_by_name() {
        let diagnostic = root_above(Path::new(ABSENT_WORKBENCH_DIR))
            .unwrap_err()
            .concat();
        assert!(diagnostic.contains(ABSENT_WORKBENCH_DIR));
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
    fn parses_separate_and_joined_dest() {
        let sep = vec![
            "--release".to_string(),
            "--dest".to_string(),
            "/x".to_string(),
        ];
        assert_eq!(parse_dest(&sep).unwrap(), Some(PathBuf::from("/x")));

        let joined = vec!["--dest=/y".to_string()];
        assert_eq!(parse_dest(&joined).unwrap(), Some(PathBuf::from("/y")));
    }

    #[test]
    fn no_dest_is_none() {
        assert_eq!(parse_dest(&["--release".to_string()]).unwrap(), None);
    }

    #[test]
    fn dest_without_value_is_an_error() {
        assert!(parse_dest(&["--dest".to_string()]).is_err());
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

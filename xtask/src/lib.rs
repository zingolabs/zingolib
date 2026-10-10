#![forbid(unsafe_code)]

pub mod ci_plan;
pub mod container;
pub mod image;
pub mod test;
pub mod workbench;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio, exit};

/// - Writes each diagnostic line to stderr as `"{prog}: {line}"`.
/// - Exits the process with 0 after `on_ok`, or 1 after the diagnostics.
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

/// - Runs `<program> <args>` as a child process, with stderr inherited, and waits for it.
pub fn stdout_of(program: &str, args: &[&str]) -> Result<String, Vec<String>> {
    stdout_in(Path::new(CURRENT_DIR), program, args, &[])
}

const CURRENT_DIR: &str = ".";

/// - Runs `<program> <args>` as a child process in `directory`, with stderr inherited, and waits for it.
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
    let output = command_in(directory, program, args, env)
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| vec![format!("failed to run {program}: {e}")])?;
    finished(program, output.status, output.stdout)
}

fn command_in(directory: &Path, program: &str, args: &[&str], env: &[(&str, &str)]) -> Command {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(directory)
        .envs(env.iter().copied());
    command
}

/// - Replaces this process with `<program> <args>` run in `directory`, so the command's exit status becomes this process's.
pub fn exec_in(
    directory: &Path,
    program: &str,
    args: &[&str],
    env: &[(&str, &str)],
) -> Result<(), Vec<String>> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command_in(directory, program, args, env).exec();
        Err(vec![format!("failed to run {program}: {error}")])
    }
    #[cfg(not(unix))]
    {
        run_streaming_in(directory, program, args, env)
    }
}

/// - Runs `<program> <args>` as a child process in `directory`, writes `input` to its stdin, and waits for it.
pub fn stdout_from_stdin(
    directory: &Path,
    program: &str,
    args: &[&str],
    input: &[u8],
) -> Result<String, Vec<String>> {
    let mut child = command_in(directory, program, args, &[])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| vec![format!("failed to run {program}: {e}")])?;
    child
        .stdin
        .take()
        .ok_or_else(|| vec![format!("{program} took no stdin")])?
        .write_all(input)
        .map_err(|e| vec![format!("cannot write to {program}: {e}")])?;
    let output = child
        .wait_with_output()
        .map_err(|e| vec![format!("failed to wait for {program}: {e}")])?;
    let finished = finished(program, output.status, output.stdout)?;
    if !finished.status.success() {
        return Err(vec![format!("`{program} {}` failed", args.join(" "))]);
    }
    Ok(finished.stdout)
}

fn finished(program: &str, status: ExitStatus, stdout: Vec<u8>) -> Result<Finished, Vec<String>> {
    let stdout =
        String::from_utf8(stdout).map_err(|e| vec![format!("{program} output not utf-8: {e}")])?;
    Ok(Finished { status, stdout })
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

/// - Runs `<program> <args>` as a child process in `directory` with every stream inherited, and waits for it.
pub fn run_streaming_in(
    directory: &Path,
    program: &str,
    args: &[&str],
    env: &[(&str, &str)],
) -> Result<(), Vec<String>> {
    let status = command_in(directory, program, args, env)
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

/// - Runs `<program> <args>` as a child process, with stderr inherited, and waits for it.
pub fn stdout_of_owned(program: &str, args: &[String]) -> Result<String, Vec<String>> {
    stdout_of(
        program,
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    )
}

pub fn utf8(file: &Path) -> Result<&str, Vec<String>> {
    file.to_str()
        .ok_or_else(|| vec![format!("{} is not valid UTF-8", file.display())])
}

/// - Creates the parent directory of `file` and any missing ancestor on disk.
pub fn create_parent(file: &Path) -> Result<(), Vec<String>> {
    file.parent().map_or(Ok(()), |parent| {
        std::fs::create_dir_all(parent)
            .map_err(|e| vec![format!("cannot create {}: {e}", parent.display())])
    })
}

/// - Removes `directory` and everything under it from disk, if present.
/// - Creates `directory` empty on disk.
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

/// - Runs `git <args>` as a child process, with stderr inherited, and waits for it.
pub fn git(args: &[&str]) -> Result<String, Vec<String>> {
    git_in(Path::new(CURRENT_DIR), args)
}

/// - Runs `git <args>` as a child process in `directory`, with stderr inherited, and waits for it.
pub fn git_in(directory: &Path, args: &[&str]) -> Result<String, Vec<String>> {
    stdout_in(directory, GIT, args, &[])
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

const BUILT_XTASK_DIR: &str = env!("CARGO_MANIFEST_DIR");

const XTASK_RELATIVE_DIR: &str = "xtask";

pub const MANIFEST: &str = "Cargo.toml";

pub fn repo_root() -> Result<PathBuf, Vec<String>> {
    root_above(Path::new(BUILT_XTASK_DIR), XTASK_RELATIVE_DIR)
}

/// - Reads the directory entry of `<crate_dir>/Cargo.toml` from disk.
pub fn root_above(crate_dir: &Path, relative_dir: &str) -> Result<PathBuf, Vec<String>> {
    let depth = Path::new(relative_dir).components().count();
    Some(crate_dir)
        .filter(|dir| dir.ends_with(relative_dir) && dir.join(MANIFEST).is_file())
        .and_then(|dir| dir.ancestors().nth(depth))
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            vec![format!(
                "{} is not a crate directory at <zingolib>/{relative_dir}",
                crate_dir.display()
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

/// - Reads `path` from disk.
pub fn read(path: &Path) -> Result<String, Vec<String>> {
    std::fs::read_to_string(path).map_err(|e| vec![format!("cannot read {}: {e}", path.display())])
}

pub fn flag_value<'a>(args: &'a [String], flag: &str) -> Result<Option<&'a str>, Vec<String>> {
    flag_value_after(args, flag, &format!("{flag}="))
}

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

pub fn parse_dest(args: &[String]) -> Result<Option<PathBuf>, Vec<String>> {
    Ok(flag_value(args, "--dest")?.map(PathBuf::from))
}

pub const TOOLCHAIN_FILE: &str = "rust-toolchain.toml";

/// - Reads `<root>/rust-toolchain.toml` from disk.
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

fn channel_value(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("channel")?.trim_start();
    let value = rest.strip_prefix('=')?.trim_start().strip_prefix('"')?;
    let end = value.find('"')?;
    Some(value[..end].to_string())
}

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

    const ABSENT_XTASK_DIR: &str = "/absent/zingolib/xtask";

    #[test]
    fn the_root_is_the_directory_above_the_xtask_crate() {
        let xtask_dir = Path::new(BUILT_XTASK_DIR);
        assert_eq!(
            root_above(xtask_dir, XTASK_RELATIVE_DIR).unwrap(),
            xtask_dir.parent().unwrap()
        );
    }

    #[test]
    fn repo_root_is_the_root_above_the_built_crate_directory() {
        assert_eq!(
            repo_root().unwrap(),
            root_above(Path::new(BUILT_XTASK_DIR), XTASK_RELATIVE_DIR).unwrap()
        );
    }

    #[test]
    fn a_foreign_manifest_directory_in_the_environment_does_not_move_the_root() {
        let inner = Command::new(std::env::current_exe().unwrap())
            .env("CARGO_MANIFEST_DIR", ABSENT_XTASK_DIR)
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
    fn a_directory_that_is_not_the_crate_is_refused_by_name() {
        let source_dir = Path::new(BUILT_XTASK_DIR).join("src");
        let diagnostic = root_above(&source_dir, XTASK_RELATIVE_DIR)
            .unwrap_err()
            .concat();
        assert!(diagnostic.contains(source_dir.to_str().unwrap()));
    }

    #[test]
    fn a_crate_directory_that_is_absent_is_refused_by_name() {
        let diagnostic = root_above(Path::new(ABSENT_XTASK_DIR), XTASK_RELATIVE_DIR)
            .unwrap_err()
            .concat();
        assert!(diagnostic.contains(ABSENT_XTASK_DIR));
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

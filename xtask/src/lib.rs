#![forbid(unsafe_code)]

pub mod birth_trial;
pub mod bundle_nym_proxy;
pub mod ci_plan;
pub mod container;
pub mod dupes_gate;
pub mod exclusion_audit;
pub mod exit_census;
pub mod image;
pub mod run_cli;
pub mod session;
pub mod sweep_teardown;
pub mod sync_ab;
pub mod sync_bench;
pub mod test;
pub mod test_summary;
pub mod workbench;

use std::collections::BTreeSet;
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

pub fn command_in(directory: &Path, program: &str, args: &[&str], env: &[(&str, &str)]) -> Command {
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

pub const DEFAULT_BASE: &str = "origin/dev";

pub const FALLBACK_BASE: &str = "dev";

/// - Runs `git merge-base` in `root`, again against the fallback when the default base is absent.
pub fn merge_base(root: &Path, base: &str) -> Result<String, Vec<String>> {
    let found = |reference: &str| {
        git_in(root, &["merge-base", "HEAD", reference]).map(|commit| commit.trim().to_string())
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

const LOCKFILE: &str = "Cargo.lock";
pub const MANIFEST_PATH_FLAG: &str = "--manifest-path";
const LOCATE_PROJECT_ARGS: [&str; 3] = ["locate-project", "--message-format", "plain"];
const PKGID: &str = "pkgid";
pub const PACKAGE_ID_FORMAT: &str = "{p}";
const MEMBERS_ARGS: [&str; 8] = [
    "tree",
    "--workspace",
    "--depth",
    "0",
    "--prefix",
    "none",
    "--format",
    PACKAGE_ID_FORMAT,
];
const PACKAGE_DIR_OPEN: &str = " (";
const PACKAGE_DIR_CLOSE: char = ')';

/// - Runs `cargo locate-project` as a child process in `dir`.
pub fn manifest_above(dir: &Path) -> Result<PathBuf, Vec<String>> {
    stdout_in(dir, CARGO, &LOCATE_PROJECT_ARGS, &[]).map(|path| PathBuf::from(path.trim()))
}

/// - Runs `cargo pkgid` as a child process with both output streams discarded.
pub fn declares_a_package(manifest: &Path) -> Result<bool, Vec<String>> {
    Command::new(CARGO)
        .args([PKGID, MANIFEST_PATH_FLAG])
        .arg(manifest)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .map_err(|e| vec![format!("failed to run {CARGO}: {e}")])
}

pub fn package_location(package: &str) -> Option<(&str, &Path)> {
    let (head, rest) = package.split_once(PACKAGE_DIR_OPEN)?;
    let dir = rest.strip_suffix(PACKAGE_DIR_CLOSE)?;
    let name = head.split_whitespace().next()?;
    Some((name, Path::new(dir)))
}

/// - Runs `cargo tree` as a child process.
pub fn workspace_members(manifest: &Path) -> Result<Vec<PathBuf>, Vec<String>> {
    let args = [
        MEMBERS_ARGS.as_slice(),
        &[MANIFEST_PATH_FLAG, utf8(manifest)?],
    ]
    .concat();
    let listing = stdout_of(CARGO, &args)?;
    Ok(listing
        .lines()
        .filter_map(package_location)
        .map(|(_, dir)| dir.join(MANIFEST))
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

/// - Runs `git diff` in `root`, then `cargo locate-project` and `cargo pkgid` once per changed directory.
pub fn touched_manifests(root: &Path, merge_base: &str) -> Result<Vec<PathBuf>, Vec<String>> {
    let changed = git_in(root, &["diff", "--name-only", merge_base])?;
    let dirs: BTreeSet<PathBuf> = changed
        .lines()
        .map(|file| existing_dir_of(root, Path::new(file)))
        .collect();
    let mut manifests = BTreeSet::new();
    for dir in dirs {
        let manifest = manifest_above(&dir)?;
        let beside = manifest.parent() == Some(dir.as_path());
        if beside || declares_a_package(&manifest)? {
            manifests.insert(manifest);
        }
    }
    Ok(manifests.into_iter().collect())
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

const BUILT_XTASK_DIR: &str = env!("CARGO_MANIFEST_DIR");

const XTASK_RELATIVE_DIR: &str = "xtask";

pub const MANIFEST: &str = "Cargo.toml";

pub const LIST_SEPARATOR: &str = ", ";

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
    String::from_utf8(read_bytes(path)?)
        .map_err(|e| vec![format!("{} is not UTF-8: {e}", path.display())])
}

/// - Reads `path` from disk.
pub fn read_bytes(path: &Path) -> Result<Vec<u8>, Vec<String>> {
    std::fs::read(path).map_err(|e| vec![format!("cannot read {}: {e}", path.display())])
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

pub fn required_flag<'a>(
    args: &'a [String],
    flag: &str,
    usage: &str,
) -> Result<&'a str, Vec<String>> {
    flag_value(args, flag)?.ok_or_else(|| vec![format!("missing {flag}"), usage.to_string()])
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
    fn a_package_location_names_the_crate_and_its_directory() {
        assert_eq!(
            package_location("zingo v2.0.0 (/w/zingo-ffi/lib)"),
            Some(("zingo", Path::new("/w/zingo-ffi/lib")))
        );
        assert_eq!(package_location("http v1.0.0"), None);
        assert_eq!(package_location(""), None);
    }

    #[test]
    fn the_manifest_above_a_nested_member_source_is_that_member() {
        let root = repo_root().unwrap();
        assert_eq!(
            manifest_above(&root.join("zingo-ffi/lib/src")).unwrap(),
            root.join("zingo-ffi/lib").join(MANIFEST)
        );
        assert_eq!(
            manifest_above(&root.join("docs")).unwrap(),
            root.join(MANIFEST)
        );
    }

    #[test]
    fn a_virtual_manifest_declares_no_package() {
        let root = repo_root().unwrap();
        assert!(!declares_a_package(&root.join(MANIFEST)).unwrap());
        assert!(declares_a_package(&root.join("zingo-cli").join(MANIFEST)).unwrap());
    }

    #[test]
    fn the_root_workspace_members_include_the_nested_ones() {
        let root = repo_root().unwrap();
        let members = workspace_members(&root.join(MANIFEST)).unwrap();
        for member in ["zingo-ffi/lib", "zingo-ffi/uniffi-bindgen", "zingo-cli"] {
            assert!(
                members.contains(&root.join(member).join(MANIFEST)),
                "{member}"
            );
        }
    }

    #[test]
    fn every_workspace_is_found_by_its_lockfile() {
        let root = repo_root().unwrap();
        let found = workspace_manifests(&root).unwrap();
        for workspace in ["", "zingo-netutils"] {
            assert!(
                found.contains(&root.join(workspace).join(MANIFEST)),
                "{workspace:?}"
            );
        }
    }

    #[test]
    fn a_deleted_file_resolves_through_its_nearest_existing_directory() {
        let root = repo_root().unwrap();
        assert_eq!(
            existing_dir_of(&root, Path::new("zingo-cli/src/gone/file.rs")),
            root.join("zingo-cli/src")
        );
        assert_eq!(existing_dir_of(&root, Path::new("README.md")), root);
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

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use workbench::{git, read, repo_root, run};

/// The subcommand and flags that mirror CI's Cargo Hack Check job.
const HACK_ARGS: [&str; 6] = [
    "hack",
    "check",
    "--feature-powerset",
    "--lib",
    "--bins",
    "--tests",
];

/// The build directory the sweep keeps apart from an ordinary `cargo check`.
const SWEEP_TARGET_DIR: &str = "target/hack";

/// The branch a sweep compares against when the caller names no base.
const DEFAULT_BASE: &str = "origin/dev";

/// The fallback base for a checkout whose remote branch is absent.
const FALLBACK_BASE: &str = "dev";

/// What the caller asked the sweep to cover.
enum Scope {
    /// Every workspace member, in every feature combination.
    Workspace,
    /// One manifest per crate the branch touches.
    Touched(Vec<PathBuf>),
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run("feature-sweep", || sweep(&args), |()| {})
}

/// Checks each selected crate in every feature combination, failing on the first refusal.
fn sweep(args: &[String]) -> Result<(), Vec<String>> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_usage();
        return Ok(());
    }

    let root = repo_root()?;
    require_cargo_hack()?;

    match scope(&root, args)? {
        Scope::Workspace => {
            println!("feature-sweep: the whole workspace, every feature combination");
            check(&root, &["--workspace".to_string()])
        }
        Scope::Touched(manifests) if manifests.is_empty() => {
            println!("feature-sweep: no crate is touched; nothing to check");
            Ok(())
        }
        Scope::Touched(manifests) => {
            for manifest in &manifests {
                println!("feature-sweep: {}", display_relative(&root, manifest));
            }
            for manifest in &manifests {
                let path = manifest.to_string_lossy().to_string();
                check(&root, &["--manifest-path".to_string(), path])?;
            }
            Ok(())
        }
    }
}

/// Prints how to call the sweep and what each argument selects.
fn print_usage() {
    println!("usage: feature-sweep [--all] [--base <ref>] [<crate-dir>...]");
    println!();
    println!("Checks crates in every feature combination, the way CI's Cargo Hack");
    println!("Check job does, so a build that only a non-default feature compiles");
    println!("fails here rather than after the push.");
    println!();
    println!("  --all          check every workspace member (CI's own command)");
    println!("  --base <ref>   compare against <ref> instead of {DEFAULT_BASE}");
    println!("  <crate-dir>    check these crates and no others");
    println!();
    println!("With no argument the sweep checks the crates this branch touches.");
}

/// Reads the arguments into the set of crates the sweep will check.
fn scope(root: &Path, args: &[String]) -> Result<Scope, Vec<String>> {
    let request = parse(args)?;
    if request.all {
        return Ok(Scope::Workspace);
    }

    if !request.crates.is_empty() {
        let mut manifests = Vec::new();
        for name in &request.crates {
            let manifest = root.join(name).join("Cargo.toml");
            if !manifest.is_file() {
                return Err(vec![format!("no crate at {}", manifest.display())]);
            }
            manifests.push(manifest);
        }
        return Ok(Scope::Touched(manifests));
    }

    Ok(Scope::Touched(touched_manifests(root, &request.base)?))
}

/// What one command line asks the sweep to do.
#[derive(Debug, PartialEq)]
struct Request {
    all: bool,
    base: String,
    crates: Vec<String>,
}

/// Reads a command line, refusing an unknown flag or a `--base` without its reference.
fn parse(args: &[String]) -> Result<Request, Vec<String>> {
    let mut request = Request {
        all: false,
        base: DEFAULT_BASE.to_string(),
        crates: Vec::new(),
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--" {
            continue;
        } else if arg == "--all" {
            request.all = true;
        } else if let Some(reference) = arg.strip_prefix("--base=") {
            request.base = reference.to_string();
        } else if arg == "--base" {
            request.base = iter
                .next()
                .cloned()
                .ok_or_else(|| vec!["--base requires a reference argument".to_string()])?;
        } else if arg.starts_with('-') {
            return Err(vec![
                format!("unknown argument '{arg}'"),
                "run `feature-sweep --help` for the arguments it takes".to_string(),
            ]);
        } else {
            request.crates.push(arg.clone());
        }
    }
    Ok(request)
}

/// One manifest per crate whose files differ from the merge base.
fn touched_manifests(root: &Path, base: &str) -> Result<Vec<PathBuf>, Vec<String>> {
    let merge_base = merge_base(base)?;
    let changed = git(&["diff", "--name-only", &merge_base])?;
    let mut manifests = BTreeSet::new();
    for file in changed.lines() {
        if let Some(manifest) = nearest_manifest(Path::new(file), |dir| {
            root.join(dir).join("Cargo.toml").is_file()
        }) {
            let manifest = root.join(manifest);
            if declares_a_package(&read(&manifest)?) {
                manifests.insert(manifest);
            }
        }
    }
    Ok(manifests.into_iter().collect())
}

/// Reports whether a manifest carries a crate of its own rather than only a workspace.
fn declares_a_package(manifest: &str) -> bool {
    manifest
        .lines()
        .any(|line| line.trim_start().starts_with("[package]"))
}

/// The commit this branch grew from, tried against the named base then its local branch.
fn merge_base(base: &str) -> Result<String, Vec<String>> {
    if let Ok(commit) = git(&["merge-base", "HEAD", base]) {
        return Ok(commit.trim().to_string());
    }
    if base == DEFAULT_BASE {
        if let Ok(commit) = git(&["merge-base", "HEAD", FALLBACK_BASE]) {
            return Ok(commit.trim().to_string());
        }
    }
    Err(vec![
        format!("no merge base between HEAD and '{base}'"),
        "name a base that exists with --base <ref>".to_string(),
    ])
}

/// The manifest of the innermost crate directory holding this file.
fn nearest_manifest(file: &Path, has_manifest: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let mut dir = file.parent();
    while let Some(candidate) = dir {
        if has_manifest(candidate) {
            return Some(candidate.join("Cargo.toml"));
        }
        dir = candidate.parent();
    }
    None
}

/// Refuses early when cargo-hack is absent, naming the one command that installs it.
fn require_cargo_hack() -> Result<(), Vec<String>> {
    let found = Command::new("cargo")
        .args(["hack", "--version"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if found {
        return Ok(());
    }
    Err(vec![
        "cargo-hack is not installed".to_string(),
        "install it with `cargo install cargo-hack`".to_string(),
    ])
}

/// Runs one cargo-hack check, reporting the command a reader can repeat by hand.
fn check(root: &Path, selection: &[String]) -> Result<(), Vec<String>> {
    let mut command = Command::new("cargo");
    command
        .current_dir(root)
        .args(HACK_ARGS)
        .args(selection)
        .args(["--target-dir", SWEEP_TARGET_DIR]);

    let status = command
        .status()
        .map_err(|e| vec![format!("failed to run cargo hack: {e}")])?;
    if status.success() {
        return Ok(());
    }
    Err(vec![
        format!("cargo hack refused {}", selection.join(" ")),
        format!(
            "repeat it with `cargo {} {} --target-dir {SWEEP_TARGET_DIR}`",
            HACK_ARGS.join(" "),
            selection.join(" ")
        ),
    ])
}

/// The path as the repository sees it, or the whole path when it lies outside.
fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_manifest_climbs_to_the_innermost_crate() {
        let crates = |dir: &Path| matches!(dir.to_str(), Some("zingo-cli") | Some(""));
        assert_eq!(
            nearest_manifest(Path::new("zingo-cli/src/tests.rs"), crates),
            Some(PathBuf::from("zingo-cli/Cargo.toml"))
        );
    }

    #[test]
    fn nearest_manifest_ignores_a_file_under_no_crate() {
        assert_eq!(
            nearest_manifest(Path::new("docs/adr/0011.md"), |_| false),
            None
        );
    }

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn an_empty_command_line_sweeps_the_touched_crates_against_the_default_base() {
        assert_eq!(
            parse(&[]).unwrap(),
            Request {
                all: false,
                base: DEFAULT_BASE.to_string(),
                crates: Vec::new(),
            }
        );
    }

    #[test]
    fn base_parses_in_both_spellings() {
        assert_eq!(parse(&words("--base main")).unwrap().base, "main");
        assert_eq!(parse(&words("--base=main")).unwrap().base, "main");
    }

    #[test]
    fn a_base_value_is_not_read_as_a_crate() {
        assert!(parse(&words("--base main")).unwrap().crates.is_empty());
        assert_eq!(
            parse(&words("--base main zingo-cli")).unwrap().crates,
            vec!["zingo-cli".to_string()]
        );
    }

    #[test]
    fn base_without_a_reference_is_an_error() {
        assert!(parse(&words("--base")).is_err());
    }

    #[test]
    fn an_unknown_flag_is_an_error() {
        assert!(parse(&words("--every-feature")).is_err());
    }

    #[test]
    fn all_selects_the_whole_workspace() {
        assert!(parse(&words("--all")).unwrap().all);
    }

    #[test]
    fn a_lone_separator_passes_through_from_cargo_make() {
        assert_eq!(
            parse(&words("-- zingo-cli")).unwrap().crates,
            vec!["zingo-cli".to_string()]
        );
    }

    #[test]
    fn a_virtual_workspace_manifest_carries_no_package() {
        let virtual_root = "[workspace]\nmembers = [\"zingolib\"]\nresolver = \"2\"\n";
        assert!(!declares_a_package(virtual_root));
    }

    #[test]
    fn a_crate_manifest_carries_a_package() {
        let crate_manifest = "# a comment\n[package]\nname = \"zingo-cli\"\n";
        assert!(declares_a_package(crate_manifest));
    }
}

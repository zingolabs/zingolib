#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

const HACK: &str = "hack";
const INSTALL_HACK: &str = "cargo install cargo-hack";

/// The subcommand and flags that mirror CI's Cargo Hack Check job.
const HACK_ARGS: [&str; 6] = [
    HACK,
    "check",
    "--feature-powerset",
    "--lib",
    "--bins",
    "--tests",
];

/// The build directory the sweep keeps apart from an ordinary `cargo check`.
const SWEEP_TARGET_DIR: &str = "target/hack";

/// What the caller asked the sweep to cover.
enum Scope {
    /// Every workspace member, in every feature combination.
    Workspace,
    /// One manifest per crate the branch touches.
    Touched(Vec<PathBuf>),
}

fn main() -> ! {
    workbench::dispatch_from_root("feature-sweep", sweep)
}

/// Checks each selected crate in every feature combination, failing on the first refusal.
fn sweep(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_usage();
        return Ok(());
    }

    workbench::cargo_subcommand_version(HACK, INSTALL_HACK)?;

    match scope(root, args)? {
        Scope::Workspace => {
            println!("feature-sweep: the whole workspace, every feature combination");
            check(root, &["--workspace".to_string()])
        }
        Scope::Touched(manifests) if manifests.is_empty() => {
            println!("feature-sweep: no crate is touched; nothing to check");
            Ok(())
        }
        Scope::Touched(manifests) => {
            for manifest in &manifests {
                println!(
                    "feature-sweep: {}",
                    workbench::display_relative(root, manifest)
                );
            }
            for manifest in &manifests {
                let path = manifest.to_string_lossy().to_string();
                check(root, &["--manifest-path".to_string(), path])?;
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
    println!(
        "  --base <ref>   compare against <ref> instead of {}",
        workbench::DEFAULT_BASE
    );
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
            let manifest = root.join(name).join(workbench::MANIFEST);
            if !manifest.is_file() {
                return Err(vec![format!("no crate at {}", manifest.display())]);
            }
            manifests.push(manifest);
        }
        return Ok(Scope::Touched(manifests));
    }

    Ok(Scope::Touched(touched_packages(root, &request.base)?))
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
        base: workbench::DEFAULT_BASE.to_string(),
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

/// - Runs `git merge-base`, `git diff`, `cargo locate-project` and `cargo pkgid` through the library.
fn touched_packages(root: &Path, base: &str) -> Result<Vec<PathBuf>, Vec<String>> {
    workbench::touched_packages(root, &workbench::merge_base(root, base)?)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn an_empty_command_line_sweeps_the_touched_crates_against_the_default_base() {
        assert_eq!(
            parse(&[]).unwrap(),
            Request {
                all: false,
                base: workbench::DEFAULT_BASE.to_string(),
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
}

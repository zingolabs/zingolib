//! The `decent_exposure` CI gate: enforce maximum Rust item privacy across
//! every package in the repository.
//!
//! Three layers run in order, and a failing layer stops the run before the
//! next begins:
//!
//! 1. rustc's `unreachable_pub` lint, at deny: a `pub` item the crate root
//!    does not re-export must shrink to `pub(crate)` or below, so nothing is
//!    public by accident.
//! 2. rustc's `missing_docs` lint, at deny: every item that earns `pub`
//!    carries documentation, so publicity always costs a written sentence.
//! 3. `cargo public-api`: each package's public surface must match its
//!    checked-in golden under `tools/workbench/goldens/public-api/`. A surface
//!    change lands by regenerating the goldens with `--bless` and reviewing
//!    the diff like any other code. This layer alone runs under the pinned
//!    nightly named by [`GOLDEN_NIGHTLY`], because the goldens record rustdoc's
//!    rendering and that rendering is not stable across nightlies.
//!
//! The package list is derived rather than declared: the root workspace
//! members are parsed from `<root>/Cargo.toml`, and the two standalone
//! workspaces (`zingo-netutils`, `tools/workbench`) are appended. A package
//! added to the root workspace is therefore gated automatically. Each lint
//! layer runs against the package's lib target, where the public surface
//! lives; goldens pin the default-feature surface (the `nym` surface rides
//! the nym-feature job's `-D warnings` clippy instead).

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use workbench::{read, repo_root, run};

/// Workspaces that resolve outside the root lockfile. Their packages are
/// gated exactly like root members but addressed by manifest path.
const STANDALONE_WORKSPACES: &[&str] = &["zingo-netutils", "tools/workbench"];

/// The nightly toolchain that rendered the checked-in goldens.
///
/// `cargo public-api` reads rustdoc's JSON, and rustdoc renders paths into the
/// standard library differently from one nightly to the next. A nightly later
/// than this one moved the `std::io` items to `core::io` and `alloc::io`, which
/// rewrites a hundred golden lines across six packages without adding,
/// removing, or changing a single item. Following an unpinned `nightly`
/// therefore turns the gate into a calendar: it passes on the developer's
/// machine and fails on CI's fresher toolchain. Pinning the toolchain here,
/// beside the `cargo-public-api` version the CI workflow already pins, keeps a
/// golden diff a real surface change.
///
/// Bumping this pin and re-blessing the goldens belong in one commit, and the
/// CI workflow installs the same version by name.
const GOLDEN_NIGHTLY: &str = "nightly-2026-04-23";

fn main() {
    let bless = std::env::args().any(|arg| arg == "--bless");
    run(
        "decent-exposure",
        || {
            let root = repo_root()?;
            let packages = packages(&root)?;
            rustc_lint_layer(&root, &packages, "unreachable_pub")?;
            rustc_lint_layer(&root, &packages, "missing_docs")?;
            public_api_layer(&root, &packages, bless)
        },
        |()| println!("decent-exposure: every package is minimally public, documented, and pinned"),
    )
}

/// One gated package and how cargo addresses it from the repo root.
struct Package {
    /// The `[package] name`, read from the member's own manifest.
    name: String,
    /// `Some(dir)` for a standalone-workspace package (own lockfile),
    /// addressed with `--manifest-path`; `None` for a root member, addressed
    /// with `-p`.
    standalone_dir: Option<&'static str>,
}

impl Package {
    /// Start a `cargo <subcommand>` invocation addressed at this package.
    fn cargo(&self, root: &Path, subcommand: &str) -> Command {
        let mut cmd = Command::new("cargo");
        cmd.current_dir(root);
        cmd.arg(subcommand);
        match self.standalone_dir {
            Some(dir) => {
                cmd.arg("--manifest-path");
                cmd.arg(format!("{dir}/Cargo.toml"));
            }
            None => {
                cmd.args(["-p", &self.name]);
            }
        }
        cmd
    }
}

/// Every package in the repository: root workspace members plus the
/// standalone workspaces.
fn packages(root: &Path) -> Result<Vec<Package>, Vec<String>> {
    let root_manifest = read(&root.join("Cargo.toml"))?;
    let mut packages = Vec::new();
    for dir in member_dirs(&root_manifest) {
        packages.push(Package {
            name: package_name(root, &dir)?,
            standalone_dir: None,
        });
    }
    if packages.is_empty() {
        return Err(vec![
            "no members parsed from the root Cargo.toml".to_string()
        ]);
    }
    for dir in STANDALONE_WORKSPACES {
        packages.push(Package {
            name: package_name(root, dir)?,
            standalone_dir: Some(dir),
        });
    }
    Ok(packages)
}

/// Member directories of the root workspace, from its multi-line
/// `members = [ ... ]` array. The repo writes one quoted directory per line;
/// glob members would need a richer parser and are rejected by
/// [`package_name`] failing to find their manifest.
fn member_dirs(root_manifest: &str) -> Vec<String> {
    let mut dirs = Vec::new();
    let mut in_members = false;
    for line in root_manifest.lines() {
        let line = line.trim();
        if !in_members {
            in_members = line
                .strip_prefix("members")
                .map(|rest| rest.trim_start().starts_with('='))
                .unwrap_or(false);
            continue;
        }
        if line.starts_with(']') {
            break;
        }
        if let Some(dir) = quoted_value(line) {
            dirs.push(dir);
        }
    }
    dirs
}

/// The first double-quoted string in a line: `"zingolib",` → `zingolib`.
fn quoted_value(line: &str) -> Option<String> {
    let start = line.find('"')? + 1;
    let end = start + line[start..].find('"')?;
    Some(line[start..end].to_string())
}

/// The `[package] name` of the crate at `<root>/<dir>/Cargo.toml`. The first
/// `name = "..."` assignment wins, which is the `[package]` one in every
/// manifest here (`[[bin]]` tables, when present, come later).
fn package_name(root: &Path, dir: &str) -> Result<String, Vec<String>> {
    let path = root.join(dir).join("Cargo.toml");
    let manifest = read(&path)?;
    manifest
        .lines()
        .find_map(|line| {
            let rest = line.trim().strip_prefix("name")?.trim_start();
            quoted_value(rest.strip_prefix('=')?)
        })
        .ok_or_else(|| vec![format!("no package name in {}", path.display())])
}

/// Run one rustc lint at deny over every package's lib target. Every package
/// is checked before the layer fails, so one CI run shows the layer's whole
/// backlog. Findings stream straight to the log.
fn rustc_lint_layer(root: &Path, packages: &[Package], lint: &str) -> Result<(), Vec<String>> {
    println!("== decent-exposure layer: -D {lint} ==");
    let mut failed = Vec::new();
    for package in packages {
        let mut cmd = package.cargo(root, "rustc");
        cmd.args(["--lib", "--profile", "check", "--", "-D", lint]);
        let status = cmd
            .status()
            .map_err(|e| vec![format!("failed to run cargo rustc: {e}")])?;
        if !status.success() {
            failed.push(package.name.clone());
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(vec![format!("-D {lint} failed in: {}", failed.join(", "))])
    }
}

/// Diff every package's public API against its golden, or rewrite the goldens
/// under `--bless`. Runs `cargo public-api -sss` (blanket, auto-trait, and
/// auto-derived impls omitted) so a golden line is always a deliberate item,
/// under [`GOLDEN_NIGHTLY`] so a golden line always renders the same way.
fn public_api_layer(root: &Path, packages: &[Package], bless: bool) -> Result<(), Vec<String>> {
    println!("== decent-exposure layer: public API goldens ==");
    require_golden_nightly()?;
    let goldens_dir = root.join("tools/workbench/goldens/public-api");
    let mut diagnostics = Vec::new();
    for package in packages {
        let mut cmd = package.cargo(root, "public-api");
        // Both the outer cargo and the rustdoc invocation cargo public-api
        // spawns read RUSTUP_TOOLCHAIN, so this one variable puts the whole
        // rendering on the pinned nightly. Only this layer takes the override;
        // the lint layers stay on the toolchain the repository pins.
        cmd.env("RUSTUP_TOOLCHAIN", GOLDEN_NIGHTLY);
        cmd.args(["--color", "never", "-sss"]);
        let output = cmd
            .output()
            .map_err(|e| vec![format!("failed to run cargo public-api: {e}")])?;
        if !output.status.success() {
            diagnostics.push(format!(
                "{}: cargo public-api failed:\n{}",
                package.name,
                String::from_utf8_lossy(&output.stderr)
            ));
            continue;
        }
        let current = String::from_utf8(output.stdout)
            .map_err(|e| vec![format!("{}: output not utf-8: {e}", package.name)])?;
        let golden_path = goldens_dir.join(format!("{}.txt", package.name));
        if bless {
            std::fs::create_dir_all(&goldens_dir)
                .map_err(|e| vec![format!("cannot create {}: {e}", goldens_dir.display())])?;
            std::fs::write(&golden_path, &current)
                .map_err(|e| vec![format!("cannot write {}: {e}", golden_path.display())])?;
            println!("blessed {}", golden_path.display());
            continue;
        }
        match read(&golden_path) {
            Err(_) => diagnostics.push(format!(
                "{}: no golden at {}; run `cargo run --manifest-path \
                 tools/workbench/Cargo.toml --bin decent-exposure -- --bless` \
                 and commit the result",
                package.name,
                golden_path.display()
            )),
            Ok(golden) if golden != current => {
                diagnostics.extend(surface_diff(&package.name, &golden, &current));
            }
            Ok(_) => {}
        }
    }
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(diagnostics)
    }
}

/// Fail early and legibly when the pinned nightly is absent, rather than
/// letting every package report the same rustup error in turn.
fn require_golden_nightly() -> Result<(), Vec<String>> {
    let listed = Command::new("rustup")
        .args(["toolchain", "list"])
        .output()
        .map_err(|e| vec![format!("failed to run rustup: {e}")])?;
    let listed = String::from_utf8_lossy(&listed.stdout);
    if listed.lines().any(|line| line.starts_with(GOLDEN_NIGHTLY)) {
        Ok(())
    } else {
        Err(vec![format!(
            "the goldens are rendered by {GOLDEN_NIGHTLY}, which is not installed; \
             run `rustup toolchain install {GOLDEN_NIGHTLY} --profile minimal`"
        )])
    }
}

/// Order-insensitive line diff of a package's public surface: which items
/// left the golden and which joined it, each on its own diagnostic line.
fn surface_diff(name: &str, golden: &str, current: &str) -> Vec<String> {
    let golden_lines: BTreeSet<&str> = golden.lines().collect();
    let current_lines: BTreeSet<&str> = current.lines().collect();
    let mut lines = vec![format!(
        "{name}: public API differs from its golden; if intended, re-bless and review"
    )];
    for gone in golden_lines.difference(&current_lines) {
        lines.push(format!("{name}: - {gone}"));
    }
    for joined in current_lines.difference(&golden_lines) {
        lines.push(format!("{name}: + {joined}"));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_dirs_reads_the_multiline_array() {
        let manifest = r#"
[workspace]
members = [
    "libtonode-tests",
    "zingolib",
]
# trailing comment
"#;
        assert_eq!(member_dirs(manifest), vec!["libtonode-tests", "zingolib"]);
    }

    #[test]
    fn member_dirs_ignores_manifests_without_members() {
        assert!(member_dirs("[package]\nname = \"solo\"\n").is_empty());
    }

    #[test]
    fn quoted_value_takes_the_first_quoted_string() {
        assert_eq!(quoted_value("\"zingolib\",").as_deref(), Some("zingolib"));
        assert_eq!(quoted_value("no quotes"), None);
    }

    #[test]
    fn surface_diff_reports_departures_and_arrivals() {
        let diff = surface_diff(
            "pkg",
            "pub fn a()\npub fn b()\n",
            "pub fn b()\npub fn c()\n",
        );
        assert!(diff[1].contains("- pub fn a()"));
        assert!(diff[2].contains("+ pub fn c()"));
    }
}

#![forbid(unsafe_code)]
//! - Downloads the Sapling proving parameters into the user's parameter
//!   cache when they are absent, and hands their paths to rustc through
//!   `SAPLING_SPEND_PARAMS` and `SAPLING_OUTPUT_PARAMS`.
//! - Writes `git_description.rs` into `OUT_DIR`.
//! - Prints `cargo:rerun-if-changed` for this script, both parameter
//!   files, and the git state behind the descriptor; the package tree
//!   itself is never watched and never written.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::{env, fs::File, process::Command};

/// - Prints `cargo:rerun-if-changed` for this script and the git state
///   behind the descriptor.
fn register_rerun_watches() {
    println!("cargo:rerun-if-changed=build.rs");
    // The git state behind the descriptor: HEAD moves live in the
    // worktree's own git dir; tags and packed refs live in the common
    // dir (they differ in linked worktrees). The `--dirty` suffix is
    // deliberately NOT kept live — that would require watching the
    // whole tree, which is exactly the every-run rebuild this watch
    // set exists to end; it reflects the state at the last rerun.
    if let Some(git_dir) = git_path_query("--git-dir") {
        println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());
    }
    if let Some(common_dir) = git_path_query("--git-common-dir") {
        println!(
            "cargo:rerun-if-changed={}",
            common_dir.join("packed-refs").display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            common_dir.join("refs").display()
        );
    }
}

/// A path from `git rev-parse <flag>`, or `None` outside a git
/// checkout (a published-crate build), where the git watches simply
/// don't apply.
fn git_path_query(flag: &str) -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", flag])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout)
        .ok()?
        .trim_end()
        .to_string();
    if path.is_empty() {
        return None;
    }
    Some(PathBuf::from(path))
}

/// The trimmed stdout of a git command that succeeded with output, or `None`.
fn git_stdout(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|stdout| stdout.trim_end().to_string())
        .filter(|stdout| !stdout.is_empty())
}

/// The highest `zingolib_v*` tag that points at HEAD, which a depth-one fetch with tags carries.
fn tag_at_head() -> Option<String> {
    git_stdout(&[
        "tag",
        "--points-at",
        "HEAD",
        "--list",
        "zingolib_v*",
        "--sort=-version:refname",
    ])
    .and_then(|tags| tags.lines().next().map(str::to_string))
}

/// Whether a tracked file differs from HEAD, as `git describe --dirty` judges it.
fn dirty() -> bool {
    Command::new("git")
        .args(["diff-index", "--quiet", "HEAD", "--"])
        .status()
        .map(|status| !status.success())
        .unwrap_or(false)
}

/// The five-character abbreviation of HEAD.
fn hash5() -> Option<String> {
    git_stdout(&["rev-parse", "HEAD"]).map(|hash| hash.chars().take(5).collect())
}

/// `zl_<tag ver>` on a release tag, else `zl_<crate ver>_<hash5>`, with `_dirty` for a modified tree.
fn descriptor(tag: Option<&str>, crate_version: &str, hash5: Option<&str>, dirty: bool) -> String {
    let formatted = match (tag, hash5) {
        (Some(tag), _) => format!("zl_{}", tag.strip_prefix("zingolib_v").unwrap_or(tag)),
        (None, Some(hash5)) => format!("zl_{crate_version}_{hash5}"),
        (None, None) => format!("zl_{crate_version}"),
    };
    if dirty {
        format!("{formatted}_dirty")
    } else {
        formatted
    }
}

fn git_description() {
    // No network here: a build describes the state it builds from, and
    // the tags already fetched are part of that state. A tag that points
    // at HEAD needs no history, so a depth-one checkout with tags gives
    // the release form.
    let crate_version = env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let description = descriptor(
        tag_at_head().as_deref(),
        &crate_version,
        hash5().as_deref(),
        dirty(),
    );

    // Write the git description to a file which will be included in the crate
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("git_description.rs");
    let mut f = File::create(dest_path).unwrap();
    writeln!(
        f,
        "/// The build descriptor derived from the git state at compile time:\n\
        /// `zl_<ver>[_<hash5>][_dirty]`, where `<ver>` is the release tag's\n\
        /// version when the build sits exactly on a `zingolib_v<ver>` tag,\n\
        /// and otherwise the crate version followed by the abbreviated hash\n\
        pub fn git_description() -> &'static str {{\"{description}\"}}"
    )
    .unwrap();
}

/// - Downloads the Sapling parameters into the user's parameter cache
///   when they are absent; a cached pair is reused without any network.
/// - Prints `cargo:rustc-env` and `cargo:rerun-if-changed` for each
///   parameter file, so `include_bytes!` in the crate reads the cache
///   directly and a vanished file reruns this script.
/// - Panics when the download fails.
fn get_zcash_params() {
    println!("Checking if params are available...");

    let params_path = match zcash_proofs::download_sapling_parameters(Some(400)) {
        Ok(p) => {
            println!("Params downloaded!");
            println!("Spend path: {}", p.spend.to_str().unwrap());
            println!("Output path: {}", p.output.to_str().unwrap());
            p
        }
        Err(e) => {
            println!("Error downloading params: {e}");
            panic!();
        }
    };

    publish_param_path("SAPLING_SPEND_PARAMS", &params_path.spend);
    publish_param_path("SAPLING_OUTPUT_PARAMS", &params_path.output);
}

/// - Prints `cargo:rustc-env={variable}={path}` and
///   `cargo:rerun-if-changed={path}`.
fn publish_param_path(variable: &str, path: &Path) {
    let path = path.display();
    println!("cargo:rustc-env={variable}={path}");
    println!("cargo:rerun-if-changed={path}");
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    register_rerun_watches();
    get_zcash_params();
    git_description();
    Ok(())
}

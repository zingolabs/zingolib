#![forbid(unsafe_code)]
//! Build-time inputs: the Sapling proving parameters (fetched once,
//! copied beside the crate for mobile packaging) and the build descriptor
//! compiled into [`zingolib::git_description`].
//!
//! The script registers its watch set explicitly. Without any
//! `cargo:rerun-if-changed` directive cargo falls back to watching the
//! whole package tree, and this script WRITES into that tree
//! (`zcash-params/`), so the fallback made every build dirty the next
//! one: an unconditional rerun (network fetch included) plus a full
//! recompile cascade through every dependent crate, on every run.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::{env, fs::File, process::Command};

/// Register everything this script's output depends on. Emitting any
/// directive disables cargo's whole-package fallback, which is the
/// point: the package tree contains this script's own outputs.
fn register_rerun_watches() {
    println!("cargo:rerun-if-changed=build.rs");
    // The params copies: deleting either one triggers a rerun, which
    // restores it. While both exist the fetch is skipped entirely.
    println!("cargo:rerun-if-changed=zcash-params/sapling-spend.params");
    println!("cargo:rerun-if-changed=zcash-params/sapling-output.params");
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

/// Checks if zcash params are available and downloads them if not.
/// Also copies them to an internal location for use by mobile platforms.
/// Skipped entirely while both copies exist: rewriting them
/// unconditionally is what used to dirty the package on every build.
fn get_zcash_params() {
    let internal_params_path = Path::new("zcash-params");
    let spend_dest = internal_params_path.join("sapling-spend.params");
    let output_dest = internal_params_path.join("sapling-output.params");
    if spend_dest.exists() && output_dest.exists() {
        return;
    }

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

    // Copy the params to the internal location.
    std::fs::create_dir_all(internal_params_path).unwrap();
    std::fs::copy(params_path.spend, spend_dest).unwrap();
    std::fs::copy(params_path.output, output_dest).unwrap();
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    register_rerun_watches();
    get_zcash_params();
    git_description();
    Ok(())
}

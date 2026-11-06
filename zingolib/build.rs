#![forbid(unsafe_code)]
//! - Downloads the Sapling proving parameters into the user's parameter
//!   cache when they are absent, and hands their paths to rustc through
//!   `SAPLING_SPEND_PARAMS` and `SAPLING_OUTPUT_PARAMS`, unless the
//!   `runtime-sapling-params` feature is on.
//! - Writes `git_description.txt` and `git_description.rs` into `OUT_DIR`
//!   and prints `cargo:rustc-env=ZINGOLIB_DESCRIPTOR`.
//! - Prints `cargo:rerun-if-changed` for this script, both parameter
//!   files, and the git state behind the descriptor; the package tree
//!   itself is never watched and never written.

#[path = "short_commit.rs"]
mod short_commit;

use std::path::{Path, PathBuf};
use std::{env, io, process::Command};

const BUILD_SCRIPT: &str = "build.rs";
const DESCRIPTOR_TEXT_FILE: &str = "git_description.txt";
const DESCRIPTOR_ENV: &str = "ZINGOLIB_DESCRIPTOR";
const DESCRIPTOR_SOURCE_FILE: &str = "git_description.rs";
const RUNTIME_PARAMS_FEATURE: &str = "CARGO_FEATURE_RUNTIME_SAPLING_PARAMS";

/// - Prints `cargo:rerun-if-changed` for this script and the git state
///   behind the descriptor.
fn register_rerun_watches() {
    println!("cargo:rerun-if-changed={BUILD_SCRIPT}");
    // HEAD moves live in the worktree's own git dir. Branch refs and
    // packed refs live in the common dir, which differs from the git dir
    // in a linked worktree. The `_dirty` suffix reflects the tree at the
    // last rerun, because keeping it live would mean watching the whole
    // tree, and this watch set exists to end that every-run rebuild.
    if let Some(git_dir) = git_path_query("--git-dir") {
        println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());
    }
    if let Some(common_dir) = git_path_query("--git-common-dir") {
        let packed_refs = common_dir.join("packed-refs");
        if packed_refs.exists() {
            println!("cargo:rerun-if-changed={}", packed_refs.display());
        }
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

/// Whether a tracked file differs from HEAD, as `git describe --dirty` judges it.
fn dirty() -> bool {
    Command::new("git")
        .args(["diff-index", "--quiet", "HEAD", "--"])
        .status()
        .map(|status| !status.success())
        .unwrap_or(false)
}

/// - Runs `git rev-parse HEAD`.
fn short_head() -> Option<String> {
    git_stdout(&["rev-parse", "HEAD"])
        .and_then(|head| short_commit::abbreviate(&head).map(str::to_string))
}

fn descriptor(crate_version: &str, short_head: Option<&str>, dirty: bool) -> String {
    let formatted = match short_head {
        Some(short_head) => format!("zl_{crate_version}_{short_head}"),
        None => format!("zl_{crate_version}"),
    };
    if dirty {
        format!("{formatted}_dirty")
    } else {
        formatted
    }
}

/// - Runs `git rev-parse HEAD` and `git diff-index`.
/// - Prints `cargo:rustc-env=ZINGOLIB_DESCRIPTOR`.
/// - Writes `git_description.txt` and `git_description.rs` into `OUT_DIR`.
fn git_description() -> Result<(), io::Error> {
    let crate_version = env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let description = descriptor(&crate_version, short_head().as_deref(), dirty());

    println!("cargo:rustc-env={DESCRIPTOR_ENV}={description}");

    let out_dir = PathBuf::from(env::var("OUT_DIR").map_err(io::Error::other)?);
    std::fs::write(out_dir.join(DESCRIPTOR_TEXT_FILE), &description)?;
    std::fs::write(
        out_dir.join(DESCRIPTOR_SOURCE_FILE),
        format!(
            "pub fn git_description() -> &'static str {{\n    \
             include_str!(concat!(env!(\"OUT_DIR\"), \"/{DESCRIPTOR_TEXT_FILE}\"))\n\
             }}\n"
        ),
    )
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
    if env::var_os(RUNTIME_PARAMS_FEATURE).is_none() {
        get_zcash_params();
    }
    git_description()?;
    Ok(())
}

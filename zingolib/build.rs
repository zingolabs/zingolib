#![forbid(unsafe_code)]
//! Build-time inputs: the Sapling proving parameters (fetched once,
//! copied beside the crate for mobile packaging) and the `git describe`
//! string compiled into [`zingolib::git_description`].
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
    // The git state behind `git describe`: HEAD moves live in the
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

fn descriptor(raw: &str, tag_prefix: &str, part: &str) -> String {
    let (body, dirty) = match raw.strip_suffix("-dirty") {
        Some(stripped) => (stripped, true),
        None => (raw, false),
    };
    let fields: Vec<&str> = body.rsplitn(3, '-').collect();
    let formatted = match fields.as_slice() {
        [hash, count, tag]
            if hash.starts_with('g') && count.chars().all(|c| c.is_ascii_digit()) =>
        {
            let ver = tag.strip_prefix(tag_prefix).unwrap_or(tag);
            let hash5: String = hash[1..].chars().take(5).collect();
            if *count == "0" {
                format!("{part}_{ver}")
            } else {
                format!("{part}_{ver}_{count}_{hash5}")
            }
        }
        _ => {
            let hash5: String = body.chars().take(5).collect();
            format!("{part}_{hash5}")
        }
    };
    if dirty {
        format!("{formatted}_dirty")
    } else {
        formatted
    }
}

fn git_description() {
    // No network here: a build must describe the state it builds from,
    // and the tags already fetched are part of that state. (The
    // previous `git fetch --tags` on every rerun was both a per-build
    // network round-trip and a source of description drift.)
    let description = Command::new("git")
        .args([
            "describe",
            "--dirty",
            "--always",
            "--long",
            "--match=zingolib_v*",
        ])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|stdout| stdout.trim_end().to_string())
        .filter(|description| !description.is_empty())
        .map(|raw| descriptor(&raw, "zingolib_v", "zl"))
        // Outside a usable git checkout (published crate, bind-mounted
        // container workspace with unresolved ownership), fall back to
        // the crate version rather than embedding an empty string.
        .unwrap_or_else(|| format!("zl_{}", env::var("CARGO_PKG_VERSION").unwrap_or_default()));

    // Write the git description to a file which will be included in the crate
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("git_description.rs");
    let mut f = File::create(dest_path).unwrap();
    writeln!(
        f,
        "/// The build descriptor derived from 'git describe' at compile time:\n\
        /// `zl_<ver>[_<numcommit>_<hash5>][_dirty]`, where the bracketed\n\
        /// fields are elided when the build sits exactly on its\n\
        /// `zingolib_v<ver>` release tag\n\
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

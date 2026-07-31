#![forbid(unsafe_code)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::{env, fs::File, process::Command};

// Emitting any directive disables cargo's whole-package fallback, so the
// watch set must cover the uniffi scaffolding inputs (src/) as well as
// the git state behind the zm descriptor.
fn register_rerun_watches() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-env-changed=ZINGO_MOBILE_GIT_DESCRIBE");
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

// The docker build context is rust/ and carries no .git, so the host
// build script passes the raw describe output through this env var.
fn zm_description() {
    let raw = env::var("ZINGO_MOBILE_GIT_DESCRIBE")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            Command::new("git")
                .args([
                    "describe",
                    "--dirty",
                    "--always",
                    "--long",
                    "--match=zingo-*",
                ])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|stdout| stdout.trim_end().to_string())
                .filter(|description| !description.is_empty())
        });
    let description = match raw {
        Some(raw) => descriptor(&raw, "zingo-", "zm"),
        None => "zm_unknown".to_string(),
    };
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("zm_description.rs");
    let mut f = File::create(dest_path).unwrap();
    writeln!(
        f,
        "/// The zingo-mobile part of the build descriptor:\n\
        /// `zm_<tag#>[_<numcommit>_<hash5>][_dirty]`, where the bracketed\n\
        /// fields are elided when the build sits exactly on its\n\
        /// `zingo-<tag#>` release tag\n\
        pub fn zm_description() -> &'static str {{\"{description}\"}}"
    )
    .unwrap();
}

fn main() {
    register_rerun_watches();
    uniffi_build::generate_scaffolding("src/zingo.udl").expect("A valid UDL file");
    zm_description();
}

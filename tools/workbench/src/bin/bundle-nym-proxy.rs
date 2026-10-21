//! - Runs `cargo build --package zingo-netutils --features nym --bin nym-proxy` in the repo root.
//! - Copies the built binary into `--dest <dir>` through a staged rename when that directory is not cargo's own.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::Command;

use workbench::{parse_dest, repo_root, run};

const PACKAGE: &str = "zingo-netutils";
const FEATURE: &str = "nym";
const BIN: &str = "nym-proxy";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run(
        "bundle-nym-proxy",
        || bundle(&args),
        |dest| println!("{}", dest.display()),
    )
}

/// - Runs `cargo build` in the repo root.
/// - Writes `<dest>/nym-proxy.staged` and renames it over `<dest>/nym-proxy` when `--dest` names a directory other than cargo's own.
fn bundle(args: &[String]) -> Result<PathBuf, Vec<String>> {
    let release = args.iter().any(|arg| arg == "--release");
    let explicit_dest = parse_dest(args)?;
    let root = repo_root()?;
    let profile = if release { "release" } else { "debug" };
    let binary = format!("{BIN}{}", std::env::consts::EXE_SUFFIX);

    let mut build = Command::new("cargo");
    build.current_dir(&root).args([
        "build",
        "--package",
        PACKAGE,
        "--features",
        FEATURE,
        "--bin",
        BIN,
    ]);
    if release {
        build.arg("--release");
    }
    let status = build
        .status()
        .map_err(|e| vec![format!("failed to run cargo build: {e}")])?;
    if !status.success() {
        return Err(vec![format!("cargo build of {BIN} failed ({status})")]);
    }

    // Cargo writes the binary beside the wallet binaries of the same profile,
    // which is where the CLI's proxy-path resolution looks, so the default
    // run has nothing to copy.
    let source = root.join("target").join(profile).join(&binary);
    if !source.is_file() {
        return Err(vec![format!(
            "built binary not found at {}",
            source.display()
        )]);
    }
    let Some(dest_dir) = explicit_dest else {
        return Ok(source);
    };
    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| vec![format!("cannot create {}: {e}", dest_dir.display())])?;
    if source.parent() == Some(dest_dir.as_path()) {
        return Ok(source);
    }

    let dest = dest_dir.join(&binary);
    // The previous session's proxy may still be running from `dest`, and a
    // copy over a running executable fails with "text file busy". A rename
    // replaces the path atomically and leaves the running image untouched.
    let staged = dest_dir.join(format!("{binary}.staged"));
    std::fs::copy(&source, &staged).map_err(|e| {
        vec![format!(
            "cannot copy {} to {}: {e}",
            source.display(),
            staged.display()
        )]
    })?;
    std::fs::rename(&staged, &dest).map_err(|e| {
        vec![format!(
            "cannot move {} to {}: {e}",
            staged.display(),
            dest.display()
        )]
    })?;
    Ok(dest)
}

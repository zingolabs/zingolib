use std::path::{Path, PathBuf};
use std::process::Command;

use crate::parse_dest;

/// - Writes the bundled path to stdout.
pub fn dispatch(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    let release = args.iter().any(|arg| arg == "--release");
    let dest = parse_dest(args)?;
    println!("{}", bundle(root, release, dest)?.display());
    Ok(())
}

/// - Runs `cargo build` of nym-proxy in the zingo-netutils workspace as a child process.
/// - Copies the built binary beside the wallet binaries under `target/<profile>/`, or under `--dest`, replacing the path atomically.
pub fn bundle(
    root: &Path,
    release: bool,
    explicit_dest: Option<PathBuf>,
) -> Result<PathBuf, Vec<String>> {
    let profile = if release { "release" } else { "debug" };
    let binary = format!("nym-proxy{}", std::env::consts::EXE_SUFFIX);

    let manifest = root.join("zingo-netutils/Cargo.toml");
    let mut build = Command::new("cargo");
    build
        .arg("build")
        .arg("--manifest-path")
        .arg(&manifest)
        .args(["--features", "nym", "--bin", "nym-proxy"]);
    if release {
        build.arg("--release");
    }
    let status = build
        .status()
        .map_err(|e| vec![format!("failed to run cargo build: {e}")])?;
    if !status.success() {
        return Err(vec![format!("cargo build of nym-proxy failed ({status})")]);
    }

    let source = root
        .join("zingo-netutils/target")
        .join(profile)
        .join(&binary);
    if !source.is_file() {
        return Err(vec![format!(
            "built binary not found at {}",
            source.display()
        )]);
    }

    let dest_dir = explicit_dest.unwrap_or_else(|| root.join("target").join(profile));
    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| vec![format!("cannot create {}: {e}", dest_dir.display())])?;
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

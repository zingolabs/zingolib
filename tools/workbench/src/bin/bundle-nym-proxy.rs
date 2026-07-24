//! Build the standalone `nym-proxy` binary and bundle it beside the wallet.
//!
//! The Nym mixnet transport lives in the `zingo-netutils` workspace, which
//! carries its own lockfile (ADR 0011): the nym-sdk stack cannot share the main
//! lock, so an ordinary `cargo build -p zingo-cli` never produces `nym-proxy`.
//! This tool builds it from that separate workspace and copies it next to the
//! main `target/<profile>/` binaries, where the CLI's proxy-path resolution
//! finds it with no configuration (`resolve_proxy_path` in zingo-cli).
//!
//! Usage: `bundle-nym-proxy [--release] [--dest <dir>]`. Without `--dest` the
//! binary is placed in `target/<profile>/` under the repo root.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::Command;

use workbench::{repo_root, run};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run(
        "bundle-nym-proxy",
        || bundle(&args),
        |dest| println!("{}", dest.display()),
    )
}

/// Build the `nym-proxy` binary from the `zingo-netutils` workspace and copy it
/// to the destination, returning the bundled path.
fn bundle(args: &[String]) -> Result<PathBuf, Vec<String>> {
    let release = args.iter().any(|arg| arg == "--release");
    let explicit_dest = parse_dest(args)?;
    let root = repo_root()?;
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
    std::fs::copy(&source, &dest).map_err(|e| {
        vec![format!(
            "cannot copy {} to {}: {e}",
            source.display(),
            dest.display()
        )]
    })?;
    Ok(dest)
}

/// The value of a `--dest <dir>` or `--dest=<dir>` argument, if present.
fn parse_dest(args: &[String]) -> Result<Option<PathBuf>, Vec<String>> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(dir) = arg.strip_prefix("--dest=") {
            return Ok(Some(PathBuf::from(dir)));
        }
        if arg == "--dest" {
            let dir = iter
                .next()
                .ok_or_else(|| vec!["--dest requires a directory argument".to_string()])?;
            return Ok(Some(PathBuf::from(dir)));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_separate_and_joined_dest() {
        let sep = vec![
            "--release".to_string(),
            "--dest".to_string(),
            "/x".to_string(),
        ];
        assert_eq!(parse_dest(&sep).unwrap(), Some(PathBuf::from("/x")));

        let joined = vec!["--dest=/y".to_string()];
        assert_eq!(parse_dest(&joined).unwrap(), Some(PathBuf::from("/y")));
    }

    #[test]
    fn no_dest_is_none() {
        assert_eq!(parse_dest(&["--release".to_string()]).unwrap(), None);
    }

    #[test]
    fn dest_without_value_is_an_error() {
        assert!(parse_dest(&["--dest".to_string()]).is_err());
    }
}

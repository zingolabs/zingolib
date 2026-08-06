//! Shared helpers for the workbench tooling crate (one binary per `src/bin/*.rs`).
//!
//! Every tool follows the same shape: resolve something under the repo root,
//! then either print a result or emit one-or-more `"{prog}: {line}"`
//! diagnostics and exit non-zero. [`run`] centralises that `main()` shape, and
//! [`repo_root`], [`git`], and [`toolchain_channel`] are the shared primitives.

#![forbid(unsafe_code)]

pub mod wallet_grammars;

use std::path::{Path, PathBuf};
use std::process::{exit, Command};

/// Run a tool `body`, reporting diagnostics as `"{prog}: {line}"` to stderr and
/// exiting `1` on error. On success runs `on_ok` (e.g. to print a result) and
/// exits `0`. This is the single `main()` shape shared by every binary.
pub fn run<T>(
    prog: &str,
    body: impl FnOnce() -> Result<T, Vec<String>>,
    on_ok: impl FnOnce(T),
) -> ! {
    match body() {
        Ok(value) => {
            on_ok(value);
            exit(0);
        }
        Err(lines) => {
            for line in lines {
                eprintln!("{prog}: {line}");
            }
            exit(1);
        }
    }
}

/// Run `git <args>` and return its stdout, or a one-line diagnostic on failure.
pub fn git(args: &[&str]) -> Result<String, Vec<String>> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| vec![format!("failed to run git: {e}")])?;
    if !output.status.success() {
        return Err(vec![format!("`git {}` failed", args.join(" "))]);
    }
    String::from_utf8(output.stdout).map_err(|e| vec![format!("git output not utf-8: {e}")])
}

/// Repository root via `git rev-parse --show-toplevel`.
pub fn repo_root() -> Result<PathBuf, Vec<String>> {
    Ok(PathBuf::from(
        git(&["rev-parse", "--show-toplevel"])?.trim(),
    ))
}

/// Read `path` to a string, or a one-line `cannot read …` diagnostic.
pub fn read(path: &Path) -> Result<String, Vec<String>> {
    std::fs::read_to_string(path).map_err(|e| vec![format!("cannot read {}: {e}", path.display())])
}

/// The value of a `--dest <dir>` or `--dest=<dir>` argument, if present.
pub fn parse_dest(args: &[String]) -> Result<Option<PathBuf>, Vec<String>> {
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

/// The pinned, validated rustc channel from `<root>/rust-toolchain.toml`.
///
/// Single source of truth for `RUST_VERSION`. Rejects any non-numeric channel
/// (`stable` / `nightly` / dated pins) so the CI image tag stays reproducible.
pub fn toolchain_channel(root: &Path) -> Result<String, Vec<String>> {
    let path = root.join("rust-toolchain.toml");
    let contents = read(&path)?;

    let Some(channel) = contents.lines().find_map(channel_value) else {
        return Err(vec![format!(
            "no [toolchain].channel in {}",
            path.display()
        )]);
    };

    if !is_concrete_numeric(&channel) {
        return Err(vec![
            format!("channel '{channel}' is not a concrete numeric version (e.g. 1.91 or 1.91.0)"),
            format!(
                "a pinned rustc is required; set channel = \"<x.y[.z]>\" in {}",
                path.display()
            ),
        ]);
    }
    Ok(channel)
}

/// Value of a `channel = "..."` line, mirroring `^[[:space:]]*channel[[:space:]]*=`.
/// `None` for comments, other keys, or a line without a double-quoted value.
fn channel_value(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("channel")?.trim_start();
    let value = rest.strip_prefix('=')?.trim_start().strip_prefix('"')?;
    let end = value.find('"')?;
    Some(value[..end].to_string())
}

/// `^[0-9]+\.[0-9]+(\.[0-9]+)?$`, two or three dot-separated all-digit parts.
fn is_concrete_numeric(channel: &str) -> bool {
    let parts: Vec<&str> = channel.split('.').collect();
    matches!(parts.len(), 2 | 3)
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_value_recognises_only_quoted_assignments() {
        assert_eq!(
            channel_value("channel = \"1.91.0\"").as_deref(),
            Some("1.91.0")
        );
        assert_eq!(channel_value("  channel=\"1.91\"").as_deref(), Some("1.91"));
        assert_eq!(channel_value("# channel = \"x\""), None);
        assert_eq!(channel_value("components = [\"clippy\"]"), None);
        assert_eq!(channel_value("[toolchain]"), None);
    }

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

    #[test]
    fn numeric_validation_matches_x_y_z() {
        assert!(is_concrete_numeric("1.91.0"));
        assert!(is_concrete_numeric("1.91"));
        assert!(!is_concrete_numeric("stable"));
        assert!(!is_concrete_numeric("nightly"));
        assert!(!is_concrete_numeric("1"));
        assert!(!is_concrete_numeric("1.91.0.1"));
        assert!(!is_concrete_numeric("1..0"));
    }
}

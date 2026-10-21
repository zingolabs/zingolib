//! - Runs `cargo build --package zingo-netutils --features nym --bin nym-proxy` in the repo root.
//! - Copies the built binary into `--dest <dir>` through a staged rename when that directory is not cargo's own.

#![forbid(unsafe_code)]

use std::io::BufReader;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use cargo_metadata::Message;
use workbench::{binding_layer, parse_dest, repo_root, run};

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

/// - Runs `cargo build` in the repo root and reads the executable path from its JSON messages.
/// - Writes `<dest>/nym-proxy.staged` and renames it over `<dest>/nym-proxy` when `--dest` names a directory other than cargo's own.
fn bundle(args: &[String]) -> Result<PathBuf, Vec<String>> {
    let release = args.iter().any(|arg| arg == "--release");
    let explicit_dest = parse_dest(args)?;
    let root = repo_root()?;

    // Cargo decides the output directory from CARGO_TARGET_DIR and its
    // build.target-dir configuration, so the tool reads the executable path
    // from cargo's own report instead of guessing `target/<profile>`.
    let mut build = Command::new("cargo");
    build.current_dir(&root).args([
        "build",
        "--package",
        PACKAGE,
        "--features",
        FEATURE,
        "--bin",
        BIN,
        binding_layer::MESSAGE_FORMAT_FLAG,
    ]);
    if release {
        build.arg("--release");
    }
    build.stdout(Stdio::piped());
    let mut child = build
        .spawn()
        .map_err(|e| vec![format!("failed to run cargo build: {e}")])?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| vec!["cargo build opened no stdout".to_string()])?;
    let mut executable = None;
    for message in Message::parse_stream(BufReader::new(stdout)) {
        let message = message.map_err(|e| vec![format!("cannot read a cargo message: {e}")])?;
        if let Message::CompilerArtifact(artifact) = message {
            if artifact.target.name == BIN {
                if let Some(path) = artifact.executable {
                    executable = Some(path.into_std_path_buf());
                }
            }
        }
    }
    let status = child
        .wait()
        .map_err(|e| vec![format!("failed to wait for cargo build: {e}")])?;
    if !status.success() {
        return Err(vec![format!("cargo build of {BIN} failed ({status})")]);
    }
    let source =
        executable.ok_or_else(|| vec![format!("cargo build reported no executable for {BIN}")])?;

    // Cargo writes the binary beside the wallet binaries of the same profile,
    // which is where the CLI's proxy-path resolution looks, so the default
    // run has nothing to copy.
    let Some(dest_dir) = explicit_dest else {
        return Ok(source);
    };
    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| vec![format!("cannot create {}: {e}", dest_dir.display())])?;
    if source.parent() == Some(dest_dir.as_path()) {
        return Ok(source);
    }

    let binary = source
        .file_name()
        .ok_or_else(|| vec![format!("{} has no file name", source.display())])?;
    let dest = dest_dir.join(binary);
    // The previous session's proxy may still be running from `dest`, and a
    // copy over a running executable fails with "text file busy". A rename
    // replaces the path atomically and leaves the running image untouched.
    let staged = dest_dir.join(format!("{}.staged", binary.to_string_lossy()));
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

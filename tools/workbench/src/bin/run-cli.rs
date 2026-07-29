//! Build and launch `zingo-cli`, optionally with a `nym-proxy` sidecar.
//!
//! Usage: `run-cli [--nym] [--release] [<zingo-cli args...>]`. The `--nym`
//! and `--release` flags are consumed wherever they appear; every other
//! argument is forwarded to `zingo-cli` unchanged.
//!
//! The launched session decides its own connectivity: first boot is offline,
//! and only a consent act — a stored standing Connectivity Consent from a
//! previous run, or an explicit `--online`/`--server` passed through in the
//! trailing arguments — takes it online (ADR 0025). Neither this tool nor a
//! running proxy implies consent: the CLI launches offline beside a live
//! sidecar exactly as it does without one.
//!
//! `--nym` compiles the mixnet transport into the CLI, bundles the
//! `nym-proxy` binary beside it (so the CLI's proxy-path resolution finds it
//! when Mixnet Mode is enabled), and launches one `nym-proxy` process
//! alongside the CLI, logging to `target/nym-proxy.log`. The sidecar is
//! killed when the CLI exits, so no orphan survives the session.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::{exit, Child, Command, Stdio};

use workbench::repo_root;

const PROG: &str = "run-cli";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match launch(&args) {
        Ok(code) => exit(code),
        Err(lines) => {
            for line in lines {
                eprintln!("{PROG}: {line}");
            }
            exit(1);
        }
    }
}

/// Build the CLI (and, under `--nym`, bundle and launch the proxy sidecar),
/// run the CLI to completion, and return its exit code.
fn launch(args: &[String]) -> Result<i32, Vec<String>> {
    let nym = args.iter().any(|arg| arg == "--nym");
    let release = args.iter().any(|arg| arg == "--release");
    let cli_args: Vec<&String> = args
        .iter()
        .filter(|arg| *arg != "--nym" && *arg != "--release")
        .collect();

    let root = repo_root()?;
    let profile = if release { "release" } else { "debug" };

    let mut build = Command::new("cargo");
    build.current_dir(&root).args(["build", "-p", "zingo-cli"]);
    if release {
        build.arg("--release");
    }
    if nym {
        build.args(["--features", "nym"]);
    }
    let status = build
        .status()
        .map_err(|e| vec![format!("failed to run cargo build: {e}")])?;
    if !status.success() {
        return Err(vec![format!("cargo build of zingo-cli failed ({status})")]);
    }

    let mut sidecar = if nym {
        Some(launch_proxy_sidecar(&root, release)?)
    } else {
        None
    };

    let cli = root
        .join("target")
        .join(profile)
        .join(format!("zingo-cli{}", std::env::consts::EXE_SUFFIX));
    let cli_status = Command::new(&cli)
        .args(&cli_args)
        .status()
        .map_err(|e| vec![format!("failed to launch {}: {e}", cli.display())]);

    if let Some(proxy) = sidecar.as_mut() {
        proxy.kill().ok();
        proxy.wait().ok();
        eprintln!("{PROG}: nym-proxy sidecar stopped with the session");
    }

    Ok(cli_status?.code().unwrap_or(1))
}

/// Bundle `nym-proxy` beside the wallet binaries via the sibling
/// `bundle-nym-proxy` tool, then spawn it with both output streams appended
/// to `target/nym-proxy.log`, returning the child for session-bound teardown.
fn launch_proxy_sidecar(root: &Path, release: bool) -> Result<Child, Vec<String>> {
    let mut bundle = Command::new("cargo");
    bundle.current_dir(root).args([
        "run",
        "-q",
        "--manifest-path",
        "tools/workbench/Cargo.toml",
        "--bin",
        "bundle-nym-proxy",
        "--",
    ]);
    if release {
        bundle.arg("--release");
    }
    let bundled = bundle
        .output()
        .map_err(|e| vec![format!("failed to run bundle-nym-proxy: {e}")])?;
    if !bundled.status.success() {
        return Err(vec![format!(
            "bundle-nym-proxy failed ({})",
            bundled.status
        )]);
    }
    let proxy_path = PathBuf::from(
        String::from_utf8(bundled.stdout)
            .map_err(|e| vec![format!("bundle-nym-proxy output not utf-8: {e}")])?
            .trim(),
    );

    let log_path = root.join("target").join("nym-proxy.log");
    let log = std::fs::File::create(&log_path)
        .map_err(|e| vec![format!("cannot create {}: {e}", log_path.display())])?;
    let log_for_stderr = log
        .try_clone()
        .map_err(|e| vec![format!("cannot clone log handle: {e}")])?;
    let child = Command::new(&proxy_path)
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_for_stderr))
        .spawn()
        .map_err(|e| vec![format!("failed to launch {}: {e}", proxy_path.display())])?;
    eprintln!(
        "{PROG}: nym-proxy sidecar launched (pid {}); its SOCKS5_ADDR line will \
         appear in {}. The session it runs beside still starts offline unless a \
         stored Connectivity Consent takes it online.",
        child.id(),
        log_path.display()
    );
    Ok(child)
}

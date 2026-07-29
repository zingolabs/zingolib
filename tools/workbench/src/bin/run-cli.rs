//! Build and launch `zingo-cli`, by default mixnet-capable.
//!
//! Usage: `run-cli [--clearnet] [--release] [<zingo-cli args...>]`. The
//! `--clearnet` and `--release` flags are consumed wherever they appear
//! (as is the retired `--nym`, a no-op now that it names the default);
//! every other argument is forwarded to `zingo-cli` unchanged.
//!
//! The default run compiles the mixnet transport into the CLI and bundles
//! the `nym-proxy` binary beside it, where the CLI's provisioning
//! precedence resolves it. This tool never launches the proxy: the CLI
//! owns that lifecycle, spawning the proxy at its go-online moment for an
//! Online session (Mixnet Mode forced on, ADR 0024) and never for an
//! offline one — an offline session boots no proxy at all (ADR 0025).
//! `--clearnet` opts out of both the feature and the bundling: a plain
//! build with no mixnet capability.
//!
//! The launched session decides its own connectivity: first boot is offline,
//! and only a consent act — a stored standing Connectivity Consent from a
//! previous run, or an explicit `--online`/`--server` passed through in the
//! trailing arguments — takes it online (ADR 0025). Neither this tool nor a
//! bundled proxy binary implies consent.

#![forbid(unsafe_code)]

use std::path::Path;
use std::process::{Command, exit};

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

/// Build the CLI (and, unless `--clearnet` opts out, bundle the proxy
/// binary beside it), run the CLI to completion, and return its exit code.
fn launch(args: &[String]) -> Result<i32, Vec<String>> {
    let clearnet = args.iter().any(|arg| arg == "--clearnet");
    let nym_flag = args.iter().any(|arg| arg == "--nym");
    if clearnet && nym_flag {
        return Err(vec![
            "--clearnet and --nym contradict each other; pass --clearnet for a \
         plain build, or nothing for the mixnet default"
                .to_string(),
        ]);
    }
    if nym_flag {
        eprintln!("{PROG}: note: --nym is now the default and the flag is ignored");
    }
    let nym = !clearnet;
    let release = args.iter().any(|arg| arg == "--release");
    let cli_args: Vec<&String> = args
        .iter()
        .filter(|arg| *arg != "--nym" && *arg != "--release" && *arg != "--clearnet")
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

    if nym {
        bundle_proxy(&root, release)?;
    }

    let cli = root
        .join("target")
        .join(profile)
        .join(format!("zingo-cli{}", std::env::consts::EXE_SUFFIX));
    let cli_status = Command::new(&cli)
        .args(&cli_args)
        .status()
        .map_err(|e| vec![format!("failed to launch {}: {e}", cli.display())]);

    Ok(cli_status?.code().unwrap_or(1))
}

/// Bundle `nym-proxy` beside the wallet binaries via the sibling
/// `bundle-nym-proxy` tool, so the CLI's provisioning precedence finds it.
/// The CLI decides whether the proxy ever runs: it spawns it only at an
/// Online session's go-online moment, never for an offline session.
fn bundle_proxy(root: &Path, release: bool) -> Result<(), Vec<String>> {
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
    let proxy_path = String::from_utf8(bundled.stdout)
        .map_err(|e| vec![format!("bundle-nym-proxy output not utf-8: {e}")])?;
    eprintln!(
        "{PROG}: nym-proxy bundled at {}; the CLI spawns it only for an Online \
         session (offline sessions boot no proxy)",
        proxy_path.trim()
    );
    Ok(())
}

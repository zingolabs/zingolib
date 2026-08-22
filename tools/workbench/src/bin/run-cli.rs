//! Build and launch `zingo-cli`, by default mixnet-capable.
//!
//! Usage: `run-cli [--clearnet] [--debug] [--features <list>]
//! [--target-dir <dir>] [--build-only] [<zingo-cli args...>]`. Those flags
//! are consumed wherever they appear (as are the retired `--nym` and
//! `--release`, no-ops now that they name the defaults); every other
//! argument is forwarded to `zingo-cli` unchanged. The build is a release
//! build unless `--debug` asks otherwise. `--build-only` stops after the
//! build and the bundling, which is how a caller pays the compile once
//! before timing anything, and `--target-dir` gives a build its own
//! directory, which is how two differently-featured builds stand side by
//! side without either rebuilding the other.
//!
//! The default run builds the CLI with its default features — which carry
//! the mixnet transport (ADR 0026) — and bundles the `nym-proxy` binary
//! beside it, where the CLI's provisioning precedence resolves it. This
//! tool never launches the proxy: the CLI owns that lifecycle, spawning
//! the proxy at its go-online moment for an Online session (Mixnet Mode
//! forced on, ADR 0024) and never for an offline one — an offline session
//! boots no proxy at all (ADR 0025). `--clearnet` opts out of both the
//! default features and the bundling: a plain build with no mixnet
//! capability.
//!
//! The launched session decides its own connectivity: first boot is offline,
//! and only a consent act — a stored standing Connectivity Consent from a
//! previous run, an explicit `--online`/`--server` passed through in the
//! trailing arguments, or the in-session `network on` command (ADR 0026) —
//! takes it online (ADR 0025). Neither this tool nor a bundled proxy binary
//! implies consent.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::{exit, Command};

use workbench::repo_root;

const PROG: &str = "run-cli";

/// The flag naming the cargo features the build adds.
const FEATURES_FLAG: &str = "--features";

/// The flag naming the directory the build writes its artifacts to.
const TARGET_DIR_FLAG: &str = "--target-dir";

/// The flags this tool consumes together with the value that follows them.
const VALUE_FLAGS: [&str; 2] = [FEATURES_FLAG, TARGET_DIR_FLAG];

/// The flags this tool consumes on their own.
const BARE_FLAGS: [&str; 5] = [
    "--nym",
    "--release",
    "--clearnet",
    "--build-only",
    "--debug",
];

/// The value following `flag`, which reaches the cargo build and not the CLI.
fn value_of(args: &[String], flag: &str) -> Result<Option<String>, Vec<String>> {
    args.iter()
        .position(|arg| arg == flag)
        .map(|at| {
            args.get(at + 1)
                .cloned()
                .ok_or_else(|| vec![format!("{flag} needs a value")])
        })
        .transpose()
}

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
    let debug = args.iter().any(|arg| arg == "--debug");
    let release_flag = args.iter().any(|arg| arg == "--release");
    if debug && release_flag {
        return Err(vec![
            "--debug and --release contradict each other; pass --debug for a \
         debug build, or nothing for the release default"
                .to_string(),
        ]);
    }
    if release_flag {
        eprintln!("{PROG}: note: --release is now the default and the flag is ignored");
    }
    let nym = !clearnet;
    let release = !debug;
    let build_only = args.iter().any(|arg| arg == "--build-only");
    let features = value_of(args, FEATURES_FLAG)?;
    let target_dir = value_of(args, TARGET_DIR_FLAG)?;
    let mut skip_value = false;
    let cli_args: Vec<&String> = args
        .iter()
        .filter(|arg| {
            if skip_value {
                skip_value = false;
                return false;
            }
            if VALUE_FLAGS.contains(&arg.as_str()) {
                skip_value = true;
                return false;
            }
            !BARE_FLAGS.contains(&arg.as_str())
        })
        .collect();

    let root = repo_root()?;
    let profile = if release { "release" } else { "debug" };
    // Naming a build directory is what lets two differently-featured builds
    // stand side by side, so a caller alternating between them pays no
    // rebuild at each switch.
    let builds = target_dir
        .as_ref()
        .map_or_else(|| root.join("target"), PathBuf::from);

    let mut build = Command::new("cargo");
    build.current_dir(&root).args(["build", "-p", "zingo-cli"]);
    if target_dir.is_some() {
        build.arg(TARGET_DIR_FLAG).arg(&builds);
    }
    if release {
        build.arg("--release");
    }
    if !nym {
        // The mixnet transport rides zingo-cli's default features (ADR 0026);
        // a clearnet build is the explicit opt-out.
        build.arg("--no-default-features");
    }
    // Named by the caller, never chosen here: a feature that suspends a
    // ratified refusal is the caller's deliberate act, and burying the
    // choice in this tool would make every invocation carry it silently.
    if let Some(features) = &features {
        build.arg(FEATURES_FLAG).arg(features);
    }
    let status = build
        .status()
        .map_err(|e| vec![format!("failed to run cargo build: {e}")])?;
    if !status.success() {
        return Err(vec![format!("cargo build of zingo-cli failed ({status})")]);
    }

    let beside = builds.join(profile);
    if nym {
        bundle_proxy(&root, release, &beside)?;
    }

    if build_only {
        eprintln!(
            "{PROG}: built the {profile} CLI into {} and stopped; no session was launched",
            beside.display()
        );
        return Ok(0);
    }

    let cli = beside.join(format!("zingo-cli{}", std::env::consts::EXE_SUFFIX));
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
fn bundle_proxy(root: &Path, release: bool, beside: &Path) -> Result<(), Vec<String>> {
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
    bundle.arg("--dest").arg(beside);
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

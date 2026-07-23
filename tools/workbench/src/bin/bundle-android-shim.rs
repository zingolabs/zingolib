//! Cross-compile the mixnet proxy shim for Android and lay out its package.
//!
//! The shim (`zingo-nym-proxy-ffi`, ADR 0011 mobile amendment) lives in the
//! `zingo-netutils` workspace with its own lockfile, so ordinary builds never
//! produce it. This tool release-builds it with cargo-ndk for every Android
//! ABI zingo-mobile ships (`reactNativeArchitectures` in its
//! `gradle.properties`), regenerates the Kotlin bindings from the host cdylib
//! so the `.kt` can never drift from the `.so` set, and lays out:
//!
//! ```text
//! <dest>/jniLibs/<abi>/libzingo_nym_proxy_ffi.so   (one per ABI)
//! <dest>/kotlin/uniffi/zingo_nym_proxy_ffi/…       (generated bindings)
//! ```
//!
//! zingo-mobile copies (or points gradle at) that tree during step-3
//! packaging (#2513). Usage: `bundle-android-shim [--abi <name>]… [--dest
//! <dir>]`. Without `--abi` every shipped ABI is built; without `--dest` the
//! tree lands in `target/android-shim/` under the repo root. Requires
//! cargo-ndk, an NDK, and the matching `rustup target`s.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::Command;

use workbench::{parse_dest, repo_root, run};

/// The ABIs zingo-mobile ships, paired with the Rust triple producing each.
const SHIPPED_ABIS: [(&str, &str); 4] = [
    ("arm64-v8a", "aarch64-linux-android"),
    ("armeabi-v7a", "armv7-linux-androideabi"),
    ("x86", "i686-linux-android"),
    ("x86_64", "x86_64-linux-android"),
];

const SHIM_SO: &str = "libzingo_nym_proxy_ffi.so";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run(
        "bundle-android-shim",
        || bundle(&args),
        |dest| println!("{}", dest.display()),
    )
}

/// Build every requested ABI, regenerate the Kotlin bindings, and lay out the
/// package tree, returning its root.
fn bundle(args: &[String]) -> Result<PathBuf, Vec<String>> {
    let abis = requested_abis(args)?;
    let root = repo_root()?;
    let netutils = root.join("zingo-netutils");
    let dest = parse_dest(args)?.unwrap_or_else(|| root.join("target/android-shim"));

    for (abi, triple) in &abis {
        let status = Command::new("cargo")
            .current_dir(&netutils)
            .args(["ndk", "-t", abi, "build", "--release"])
            .args(["--package", "zingo-nym-proxy-ffi"])
            .status()
            .map_err(|e| vec![format!("failed to run cargo ndk: {e}")])?;
        if !status.success() {
            return Err(vec![format!("cargo ndk build for {abi} failed ({status})")]);
        }
        let source = netutils
            .join("target")
            .join(triple)
            .join("release")
            .join(SHIM_SO);
        if !source.is_file() {
            return Err(vec![format!("built {abi} shim missing at {}", source.display())]);
        }
        let abi_dir = dest.join("jniLibs").join(abi);
        std::fs::create_dir_all(&abi_dir)
            .map_err(|e| vec![format!("cannot create {}: {e}", abi_dir.display())])?;
        std::fs::copy(&source, abi_dir.join(SHIM_SO))
            .map_err(|e| vec![format!("cannot copy {abi} shim: {e}")])?;
    }

    generate_kotlin(&netutils, &dest.join("kotlin"))?;
    Ok(dest)
}

/// Regenerate the Kotlin bindings from a freshly built host cdylib, so the
/// bindings always describe the same scaffolding the ABI builds carry.
fn generate_kotlin(netutils: &std::path::Path, out_dir: &std::path::Path) -> Result<(), Vec<String>> {
    let host_build = Command::new("cargo")
        .current_dir(netutils)
        .args(["build", "--package", "zingo-nym-proxy-ffi"])
        .status()
        .map_err(|e| vec![format!("failed to run cargo build: {e}")])?;
    if !host_build.success() {
        return Err(vec![format!("host cdylib build failed ({host_build})")]);
    }
    let library = netutils.join("target/debug").join(SHIM_SO);
    let status = Command::new("cargo")
        .current_dir(netutils)
        .args(["run", "--package", "zingo-uniffi-bindgen", "--", "generate"])
        .arg("--library")
        .arg(&library)
        .args(["--language", "kotlin"])
        .arg("--out-dir")
        .arg(out_dir)
        .status()
        .map_err(|e| vec![format!("failed to run uniffi-bindgen: {e}")])?;
    if !status.success() {
        return Err(vec![format!("kotlin bindings generation failed ({status})")]);
    }
    Ok(())
}

/// The ABI list from repeatable `--abi <name>` / `--abi=<name>` arguments;
/// every shipped ABI when none is given.
fn requested_abis(args: &[String]) -> Result<Vec<(&'static str, &'static str)>, Vec<String>> {
    let mut names = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(name) = arg.strip_prefix("--abi=") {
            names.push(name.to_string());
        } else if arg == "--abi" {
            let name = iter
                .next()
                .ok_or_else(|| vec!["--abi requires an ABI name".to_string()])?;
            names.push(name.clone());
        }
    }
    if names.is_empty() {
        return Ok(SHIPPED_ABIS.to_vec());
    }
    names
        .iter()
        .map(|name| {
            SHIPPED_ABIS
                .iter()
                .find(|(abi, _)| abi == name)
                .copied()
                .ok_or_else(|| {
                    vec![format!(
                        "unknown ABI '{name}'; shipped ABIs: {}",
                        SHIPPED_ABIS.map(|(abi, _)| abi).join(", ")
                    )]
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_abi_args_selects_every_shipped_abi() {
        assert_eq!(requested_abis(&[]).unwrap(), SHIPPED_ABIS.to_vec());
    }

    #[test]
    fn abi_args_select_and_order() {
        let args = vec!["--abi".to_string(), "x86_64".to_string(), "--abi=arm64-v8a".to_string()];
        let abis = requested_abis(&args).unwrap();
        assert_eq!(
            abis.iter().map(|(abi, _)| *abi).collect::<Vec<_>>(),
            vec!["x86_64", "arm64-v8a"]
        );
    }

    #[test]
    fn unknown_abi_is_an_error() {
        assert!(requested_abis(&["--abi".to_string(), "mips".to_string()]).is_err());
    }

    #[test]
    fn abi_without_value_is_an_error() {
        assert!(requested_abis(&["--abi".to_string()]).is_err());
    }
}

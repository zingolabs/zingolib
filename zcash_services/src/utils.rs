//! Utilities module

use std::{env, path::PathBuf};

/// Returns path to cargo manifest directory (project root)
pub(crate) fn cargo_manifest_dir() -> PathBuf {
    PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("cargo manifest to resolve to pathbuf"))
}

/// Returns path to chain cache directory
pub fn chain_cache_dir() -> PathBuf {
    cargo_manifest_dir().join("chain_cache")
}

//! Regtest mode support for zingo-cli
//! This module contains all regtest-specific functionality

use std::path::PathBuf;
use zcash_local_net::LocalNet;
use zcash_local_net::indexer::lightwalletd::Lightwalletd;
use zcash_local_net::process::IsAProcess;
use zcash_local_net::validator::zcashd::Zcashd;

/// Launch a local regtest network
pub(crate) async fn launch_local_net() -> LocalNet<Zcashd, Lightwalletd> {
    LocalNet::launch_default()
        .await
        .expect("A Regtest LocalNet should've launched.")
}
/// Get the default regtest data directory
pub(crate) fn get_regtest_dir() -> PathBuf {
    // Use a temporary directory for regtest data
    // This avoids the CARGO_MANIFEST_DIR issue at runtime
    std::env::temp_dir().join("zingo-regtest")
}

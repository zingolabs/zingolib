//! Regtest mode support for zingo-cli
//! This module contains all regtest-specific functionality

use std::path::PathBuf;
use zingolib::testutils::zcash_local_net::LocalNet;
use zingolib::testutils::zcash_local_net::indexer::lightwalletd::Lightwalletd;
use zingolib::testutils::zcash_local_net::process::IsAProcess;
use zingolib::testutils::zcash_local_net::validator::zcashd::Zcashd;

/// Launch a local regtest network
pub(crate) async fn launch_local_net() -> LocalNet<Zcashd, Lightwalletd> {
    LocalNet::launch_default()
        .await
        .expect("ing to launch a Regtest LocalNet")
}
/// Get the default regtest data directory
pub(crate) fn get_regtest_dir() -> PathBuf {
    // Use a temporary directory for regtest data
    // This avoids the CARGO_MANIFEST_DIR issue at runtime
    std::env::temp_dir().join("zingo-regtest")
}

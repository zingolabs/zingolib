//! Regtest mode support for zingo-cli
//! This module contains all regtest-specific functionality

use std::path::PathBuf;
use zingo_infra_services::LocalNet;
use zingo_infra_services::indexer::{Lightwalletd, LightwalletdConfig};
use zingo_infra_services::validator::{Zcashd, ZcashdConfig};
use zingolib::testutils::scenarios::{LIGHTWALLETD_BIN, ZCASH_CLI_BIN, ZCASHD_BIN};
use zingolib::testvectors::REG_O_ADDR_FROM_ABANDONART;
use zingolib::wallet::network::ZingolibLocalNetwork;

/// Launch a local regtest network
pub(crate) async fn launch_local_net() -> LocalNet<Lightwalletd, Zcashd> {
    LocalNet::<Lightwalletd, Zcashd>::launch(
        LightwalletdConfig {
            lightwalletd_bin: LIGHTWALLETD_BIN.clone(),
            listen_port: None,
            zcashd_conf: PathBuf::new(),
            darkside: false,
        },
        ZcashdConfig {
            zcashd_bin: ZCASHD_BIN.clone(),
            zcash_cli_bin: ZCASH_CLI_BIN.clone(),
            rpc_listen_port: None,
            activation_heights: ZingolibLocalNetwork::default().into(),
            miner_address: Some(REG_O_ADDR_FROM_ABANDONART),
            chain_cache: None,
        },
    )
    .await
}
/// Get the default regtest data directory
pub(crate) fn get_regtest_dir() -> PathBuf {
    // Use a temporary directory for regtest data
    // This avoids the CARGO_MANIFEST_DIR issue at runtime
    std::env::temp_dir().join("zingo-regtest")
}

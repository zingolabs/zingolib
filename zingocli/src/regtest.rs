//! Regtest mode support for zingo-cli
//! This module contains all regtest-specific functionality

use std::path::PathBuf;
use zingo_infra_services::LocalNet;
use zingo_infra_services::indexer::{Lightwalletd, LightwalletdConfig};
use zingo_infra_services::validator::{Zcashd, ZcashdConfig};
use zingolib::testutils::scenarios::{LIGHTWALLETD_BIN, ZCASH_CLI_BIN, ZCASHD_BIN};
use zingolib::testvectors::REG_O_ADDR_FROM_ABANDONART;
use zingolib::wallet::network::ZingolibLocalNetwork;

use crate::commands::RT;

/// Launch a local regtest network
pub async fn launch_local_net() -> LocalNet<Lightwalletd, Zcashd> {
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

/// Setup regtest environment
pub fn setup_regtest(
    cli_config: &mut crate::ConfigTemplate,
) -> (
    Option<tempfile::TempDir>,
    Option<LocalNet<Lightwalletd, Zcashd>>,
) {
    use zingolib::config::ChainType;

    let wallet_dir = if matches!(cli_config.chaintype, ChainType::Regtest(_)) {
        let wallet_dir = tempfile::tempdir().unwrap();
        cli_config.data_dir = wallet_dir.path().to_path_buf();
        Some(wallet_dir)
    } else {
        None
    };

    let local_net = if matches!(cli_config.chaintype, ChainType::Regtest(_)) {
        Some(RT.block_on(launch_local_net()))
    } else {
        None
    };

    (wallet_dir, local_net)
}

/// Get the default regtest data directory
pub fn get_regtest_dir() -> PathBuf {
    // Use a temporary directory for regtest data
    // This avoids the CARGO_MANIFEST_DIR issue at runtime
    std::env::temp_dir().join("zingo-regtest")
}

/// Get the default regtest server URI
pub fn get_regtest_server() -> String {
    "http://127.0.0.1:18232".to_string()
}

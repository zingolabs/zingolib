use std::path::PathBuf;

use zcash_protocol::{PoolType, ShieldedProtocol};

use testvectors::REG_O_ADDR_FROM_ABANDONART;
use zingolib::testutils::lightclient::{from_inputs, get_base_address};

use zcash_services_utils::client;

use zcash_services::{
    indexer::{Indexer as _, Lightwalletd, LightwalletdConfig, Zainod, ZainodConfig},
    network::{self, ActivationHeights},
    utils,
    validator::{Validator, Zcashd, ZcashdConfig, Zebrad, ZebradConfig, ZEBRAD_DEFAULT_MINER},
    LocalNet,
};

const ZCASHD_BIN: Option<PathBuf> = None;
const ZCASH_CLI_BIN: Option<PathBuf> = None;
const ZEBRAD_BIN: Option<PathBuf> = None;
const LIGHTWALLETD_BIN: Option<PathBuf> = None;
const ZAINOD_BIN: Option<PathBuf> = None;

#[tokio::test]
async fn launch_zcashd() {
    tracing_subscriber::fmt().init();

    let zcashd = Zcashd::launch(ZcashdConfig {
        zcashd_bin: ZCASHD_BIN,
        zcash_cli_bin: ZCASH_CLI_BIN,
        rpc_listen_port: None,
        activation_heights: network::ActivationHeights::default(),
        miner_address: Some(REG_O_ADDR_FROM_ABANDONART),
        chain_cache: None,
    })
    .await
    .unwrap();
    zcashd.print_stdout();
    zcashd.print_stderr();
}

#[tokio::test]
async fn launch_zcashd_custom_activation_heights() {
    tracing_subscriber::fmt().init();

    let activation_heights = ActivationHeights {
        overwinter: 1.into(),
        sapling: 1.into(),
        blossom: 1.into(),
        heartwood: 1.into(),
        canopy: 3.into(),
        nu5: 5.into(),
        nu6: 7.into(),
    };
    let zcashd = Zcashd::launch(ZcashdConfig {
        zcashd_bin: ZCASHD_BIN,
        zcash_cli_bin: ZCASH_CLI_BIN,
        rpc_listen_port: None,
        activation_heights,
        miner_address: Some(REG_O_ADDR_FROM_ABANDONART),
        chain_cache: None,
    })
    .await
    .unwrap();
    zcashd.generate_blocks(8).await.unwrap();
    zcashd.print_stdout();
    zcashd.print_stderr();
}

#[tokio::test]
async fn launch_zebrad() {
    tracing_subscriber::fmt().init();

    let zebrad = Zebrad::launch(ZebradConfig {
        zebrad_bin: ZEBRAD_BIN,
        network_listen_port: None,
        rpc_listen_port: None,
        indexer_listen_port: None,
        activation_heights: network::ActivationHeights::default(),
        miner_address: ZEBRAD_DEFAULT_MINER,
        chain_cache: None,
        network: network::Network::Regtest,
    })
    .await
    .unwrap();
    zebrad.print_stdout();
    zebrad.print_stderr();
}

#[ignore = "temporary during refactor into workspace"]
#[tokio::test]
async fn launch_zebrad_with_cache() {
    tracing_subscriber::fmt().init();

    let zebrad = Zebrad::launch(ZebradConfig {
        zebrad_bin: ZEBRAD_BIN,
        network_listen_port: None,
        rpc_listen_port: None,
        indexer_listen_port: None,
        activation_heights: network::ActivationHeights::default(),
        miner_address: ZEBRAD_DEFAULT_MINER,
        chain_cache: Some(utils::chain_cache_dir().join("client_rpc_tests_large")),
        network: network::Network::Regtest,
    })
    .await
    .unwrap();
    zebrad.print_stdout();
    zebrad.print_stderr();

    assert_eq!(zebrad.get_chain_height().await, 52.into());
}

#[tokio::test]
async fn launch_localnet_zainod_zcashd() {
    tracing_subscriber::fmt().init();

    let local_net = LocalNet::<Zainod, Zcashd>::launch(
        ZainodConfig {
            zainod_bin: ZAINOD_BIN,
            listen_port: None,
            validator_port: 0,
            chain_cache: None,
            network: network::Network::Regtest,
        },
        ZcashdConfig {
            zcashd_bin: ZCASHD_BIN,
            zcash_cli_bin: ZCASH_CLI_BIN,
            rpc_listen_port: None,
            activation_heights: network::ActivationHeights::default(),
            miner_address: Some(REG_O_ADDR_FROM_ABANDONART),
            chain_cache: None,
        },
    )
    .await;

    local_net.validator().print_stdout();
    local_net.validator().print_stderr();
    local_net.indexer().print_stdout();
    local_net.indexer().print_stderr();
}

#[tokio::test]
async fn launch_localnet_zainod_zebrad() {
    tracing_subscriber::fmt().init();

    let local_net = LocalNet::<Zainod, Zebrad>::launch(
        ZainodConfig {
            zainod_bin: ZAINOD_BIN,
            listen_port: None,
            validator_port: 0,
            chain_cache: None,
            network: network::Network::Regtest,
        },
        ZebradConfig {
            zebrad_bin: ZEBRAD_BIN,
            network_listen_port: None,
            rpc_listen_port: None,
            indexer_listen_port: None,
            activation_heights: network::ActivationHeights::default(),
            miner_address: ZEBRAD_DEFAULT_MINER,
            chain_cache: None,
            network: network::Network::Regtest,
        },
    )
    .await;

    local_net.validator().print_stdout();
    local_net.validator().print_stderr();
    local_net.indexer().print_stdout();
    local_net.indexer().print_stderr();
}

#[tokio::test]
async fn launch_localnet_lightwalletd_zcashd() {
    tracing_subscriber::fmt().init();

    let local_net = LocalNet::<Lightwalletd, Zcashd>::launch(
        LightwalletdConfig {
            lightwalletd_bin: LIGHTWALLETD_BIN,
            listen_port: None,
            zcashd_conf: PathBuf::new(),
            darkside: false,
        },
        ZcashdConfig {
            zcashd_bin: ZCASHD_BIN,
            zcash_cli_bin: ZCASH_CLI_BIN,
            rpc_listen_port: None,
            activation_heights: network::ActivationHeights::default(),
            miner_address: Some(REG_O_ADDR_FROM_ABANDONART),
            chain_cache: None,
        },
    )
    .await;

    local_net.validator().print_stdout();
    local_net.validator().print_stderr();
    local_net.indexer().print_stdout();
    local_net.indexer().print_lwd_log();
    local_net.indexer().print_stderr();
}

#[tokio::test]
async fn launch_localnet_lightwalletd_zebrad() {
    tracing_subscriber::fmt().init();

    let local_net = LocalNet::<Lightwalletd, Zebrad>::launch(
        LightwalletdConfig {
            lightwalletd_bin: LIGHTWALLETD_BIN,
            listen_port: None,
            zcashd_conf: PathBuf::new(),
            darkside: false,
        },
        ZebradConfig {
            zebrad_bin: ZEBRAD_BIN,
            network_listen_port: None,
            rpc_listen_port: None,
            indexer_listen_port: None,
            activation_heights: network::ActivationHeights::default(),
            miner_address: ZEBRAD_DEFAULT_MINER,
            chain_cache: None,
            network: network::Network::Regtest,
        },
    )
    .await;

    local_net.validator().print_stdout();
    local_net.validator().print_stderr();
    local_net.indexer().print_stdout();
    local_net.indexer().print_lwd_log();
    local_net.indexer().print_stderr();
}

#[tokio::test]
async fn zainod_zcashd_basic_send() {
    tracing_subscriber::fmt().init();

    let local_net = LocalNet::<Zainod, Zcashd>::launch(
        ZainodConfig {
            zainod_bin: ZAINOD_BIN,
            listen_port: None,
            validator_port: 0,
            chain_cache: None,
            network: network::Network::Regtest,
        },
        ZcashdConfig {
            zcashd_bin: ZCASHD_BIN,
            zcash_cli_bin: ZCASH_CLI_BIN,
            rpc_listen_port: None,
            activation_heights: network::ActivationHeights::default(),
            miner_address: Some(REG_O_ADDR_FROM_ABANDONART),
            chain_cache: None,
        },
    )
    .await;

    let lightclient_dir = tempfile::tempdir().unwrap();
    let (mut faucet, mut recipient) = client::build_lightclients(
        lightclient_dir.path().to_path_buf(),
        local_net.indexer().port(),
    );
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    faucet.sync_and_await().await.unwrap();
    from_inputs::quick_send(
        &mut faucet,
        vec![(
            &get_base_address(&recipient, PoolType::Shielded(ShieldedProtocol::Orchard)).await,
            100_000,
            None,
        )],
    )
    .await
    .unwrap();
    local_net.validator().generate_blocks(1).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    faucet.sync_and_await().await.unwrap();
    recipient.sync_and_await().await.unwrap();

    let recipient_balance = recipient.do_balance().await;
    assert_eq!(recipient_balance.verified_orchard_balance, Some(100_000));

    local_net.validator().print_stdout();
    local_net.validator().print_stderr();
    local_net.indexer().print_stdout();
    local_net.indexer().print_stderr();
    println!("faucet balance:");
    println!("{:?}\n", faucet.do_balance().await);
    println!("recipient balance:");
    println!("{:?}\n", recipient_balance);
}

#[tokio::test]
async fn zainod_zebrad_basic_send() {
    tracing_subscriber::fmt().init();

    let local_net = LocalNet::<Zainod, Zebrad>::launch(
        ZainodConfig {
            zainod_bin: ZAINOD_BIN,
            listen_port: None,
            validator_port: 0,
            chain_cache: None,
            network: network::Network::Regtest,
        },
        ZebradConfig {
            zebrad_bin: ZEBRAD_BIN,
            network_listen_port: None,
            rpc_listen_port: None,
            indexer_listen_port: None,
            activation_heights: network::ActivationHeights::default(),
            miner_address: ZEBRAD_DEFAULT_MINER,
            chain_cache: None,
            network: network::Network::Regtest,
        },
    )
    .await;

    let lightclient_dir = tempfile::tempdir().unwrap();
    let (mut faucet, mut recipient) = client::build_lightclients(
        lightclient_dir.path().to_path_buf(),
        local_net.indexer().port(),
    );

    local_net.validator().generate_blocks(100).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

    faucet.sync_and_await().await.unwrap();
    faucet.quick_shield().await.unwrap();
    local_net.validator().generate_blocks(1).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    faucet.sync_and_await().await.unwrap();

    from_inputs::quick_send(
        &mut faucet,
        vec![(
            &get_base_address(&recipient, PoolType::Shielded(ShieldedProtocol::Orchard)).await,
            100_000,
            None,
        )],
    )
    .await
    .unwrap();
    local_net.validator().generate_blocks(1).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    faucet.sync_and_await().await.unwrap();
    recipient.sync_and_await().await.unwrap();

    let recipient_balance = recipient.do_balance().await;
    assert_eq!(recipient_balance.verified_orchard_balance, Some(100_000));

    local_net.validator().print_stdout();
    local_net.validator().print_stderr();
    local_net.indexer().print_stdout();
    local_net.indexer().print_stderr();
    println!("faucet balance:");
    println!("{:?}\n", faucet.do_balance().await);
    println!("recipient balance:");
    println!("{:?}\n", recipient_balance);
}

#[ignore = "lightwalletd v0.4.18+ incorrectly expects 1344-byte Equihash solutions for regtest blocks"]
#[tokio::test]
async fn lightwalletd_zcashd_basic_send() {
    tracing_subscriber::fmt().init();

    let local_net = LocalNet::<Lightwalletd, Zcashd>::launch(
        LightwalletdConfig {
            lightwalletd_bin: LIGHTWALLETD_BIN,
            listen_port: None,
            zcashd_conf: PathBuf::new(),
            darkside: false,
        },
        ZcashdConfig {
            zcashd_bin: ZCASHD_BIN,
            zcash_cli_bin: ZCASH_CLI_BIN,
            rpc_listen_port: None,
            activation_heights: network::ActivationHeights::default(),
            miner_address: Some(REG_O_ADDR_FROM_ABANDONART),
            chain_cache: None,
        },
    )
    .await;

    let lightclient_dir = tempfile::tempdir().unwrap();
    let (mut faucet, mut recipient) = client::build_lightclients(
        lightclient_dir.path().to_path_buf(),
        local_net.indexer().port(),
    );

    faucet.sync_and_await().await.unwrap();
    from_inputs::quick_send(
        &mut faucet,
        vec![(
            &get_base_address(&recipient, PoolType::Shielded(ShieldedProtocol::Orchard)).await,
            100_000,
            None,
        )],
    )
    .await
    .unwrap();
    local_net.validator().generate_blocks(1).await.unwrap();
    faucet.sync_and_await().await.unwrap();
    recipient.sync_and_await().await.unwrap();

    let recipient_balance = recipient.do_balance().await;
    assert_eq!(recipient_balance.verified_orchard_balance, Some(100_000));

    local_net.validator().print_stdout();
    local_net.validator().print_stderr();
    local_net.indexer().print_stdout();
    local_net.indexer().print_lwd_log();
    local_net.indexer().print_stderr();
    println!("faucet balance:");
    println!("{:?}\n", faucet.do_balance().await);
    println!("recipient balance:");
    println!("{:?}\n", recipient_balance);
}

#[tokio::test]
async fn lightwalletd_zebrad_basic_send() {
    tracing_subscriber::fmt().init();

    let local_net = LocalNet::<Lightwalletd, Zebrad>::launch(
        LightwalletdConfig {
            lightwalletd_bin: LIGHTWALLETD_BIN,
            listen_port: None,
            zcashd_conf: PathBuf::new(),
            darkside: false,
        },
        ZebradConfig {
            zebrad_bin: ZEBRAD_BIN,
            network_listen_port: None,
            rpc_listen_port: None,
            indexer_listen_port: None,
            activation_heights: network::ActivationHeights::default(),
            miner_address: ZEBRAD_DEFAULT_MINER,
            chain_cache: None,
            network: network::Network::Regtest,
        },
    )
    .await;

    let lightclient_dir = tempfile::tempdir().unwrap();
    let (mut faucet, mut recipient) = client::build_lightclients(
        lightclient_dir.path().to_path_buf(),
        local_net.indexer().port(),
    );

    local_net.validator().generate_blocks(100).await.unwrap();
    faucet.sync_and_await().await.unwrap();
    faucet.quick_shield().await.unwrap();
    local_net.validator().generate_blocks(1).await.unwrap();
    faucet.sync_and_await().await.unwrap();

    from_inputs::quick_send(
        &mut faucet,
        vec![(
            &get_base_address(&recipient, PoolType::Shielded(ShieldedProtocol::Orchard)).await,
            100_000,
            None,
        )],
    )
    .await
    .unwrap();
    local_net.validator().generate_blocks(1).await.unwrap();
    faucet.sync_and_await().await.unwrap();
    recipient.sync_and_await().await.unwrap();

    let recipient_balance = recipient.do_balance().await;
    assert_eq!(recipient_balance.verified_orchard_balance, Some(100_000));

    local_net.validator().print_stdout();
    local_net.validator().print_stderr();
    local_net.indexer().print_stdout();
    local_net.indexer().print_stderr();
    println!("faucet balance:");
    println!("{:?}\n", faucet.do_balance().await);
    println!("recipient balance:");
    println!("{:?}\n", recipient_balance);
}

mod client_rpcs {
    //! - In order to generate a cached blockchain from zebrad run:
    //! ```BASH
    //! ./utils/regenerate_chain_caches_report_diff.sh
    //! ```
    //! This command generates new data in the `chain_cache` directory.  The new structure should have the following added
    //!
    //! ```BASH
    //!  ├── [       4096]  client_rpc_tests_large
    //!  └── [       4096]  state
    //!      └── [       4096]  v26
    //!          └── [       4096]  regtest
    //!              ├── [     139458]  000004.log
    //!              ├── [         16]  CURRENT
    //!              ├── [         36]  IDENTITY
    //!              ├── [          0]  LOCK
    //!              ├── [     174621]  LOG
    //!              ├── [       1708]  MANIFEST-000005
    //!              ├── [     114923]  OPTIONS-000007
    //!              └── [          3]  version
    //! ```
    use zcash_services::network::Network;

    use crate::{LIGHTWALLETD_BIN, ZAINOD_BIN, ZCASHD_BIN, ZCASH_CLI_BIN, ZEBRAD_BIN};

    #[ignore = "not a test. generates chain cache for client_rpc tests."]
    #[tokio::test]
    async fn generate_zebrad_large_chain_cache() {
        tracing_subscriber::fmt().init();

        zcash_services_utils::test_fixtures::generate_zebrad_large_chain_cache(
            ZEBRAD_BIN,
            LIGHTWALLETD_BIN,
        )
        .await;
    }

    #[ignore = "not a test. generates chain cache for client_rpc tests."]
    #[tokio::test]
    async fn generate_zcashd_chain_cache() {
        tracing_subscriber::fmt().init();

        zcash_services_utils::test_fixtures::generate_zcashd_chain_cache(
            ZCASHD_BIN,
            ZCASH_CLI_BIN,
            LIGHTWALLETD_BIN,
        )
        .await;
    }

    macro_rules! rpc_fixture_test {
        ($test_name:ident) => {
            #[tokio::test]
            async fn $test_name() {
                tracing_subscriber::fmt().init();

                zcash_services_utils::test_fixtures::$test_name(
                    ZCASHD_BIN,
                    ZCASH_CLI_BIN,
                    ZAINOD_BIN,
                    LIGHTWALLETD_BIN,
                )
                .await;
            }
        };
    }
    // previously ignored
    // rpc_fixture_test!(get_block_out_of_bounds);
    // rpc_fixture_test!(get_block_range_out_of_bounds);
    // rpc_fixture_test!(send_transaction);
    // rpc_fixture_test!(get_mempool_stream_zingolib_mempool_monitor);
    // rpc_fixture_test!(get_mempool_stream);

    // slow
    // rpc_fixture_test!(get_mempool_tx);
    // rpc_fixture_test!(get_transaction);

    rpc_fixture_test!(get_lightd_info);
    rpc_fixture_test!(get_latest_block);
    rpc_fixture_test!(get_taddress_txids_all);
    rpc_fixture_test!(get_taddress_txids_lower);
    rpc_fixture_test!(get_taddress_txids_upper);
    rpc_fixture_test!(get_taddress_balance);
    rpc_fixture_test!(get_taddress_balance_stream);
    rpc_fixture_test!(get_tree_state_by_height);
    rpc_fixture_test!(get_tree_state_out_of_bounds);
    rpc_fixture_test!(get_latest_tree_state);
    rpc_fixture_test!(get_address_utxos_all);
    rpc_fixture_test!(get_address_utxos_lower);
    rpc_fixture_test!(get_address_utxos_upper);
    rpc_fixture_test!(get_address_utxos_out_of_bounds);
    rpc_fixture_test!(get_address_utxos_stream_all);
    rpc_fixture_test!(get_address_utxos_stream_lower);
    rpc_fixture_test!(get_address_utxos_stream_upper);
    rpc_fixture_test!(get_address_utxos_stream_out_of_bounds);

    // regtest_block_parse tests
    // These tests fail due to lightwalletd v0.4.18+ expecting mainnet-sized
    // Equihash solutions (1344 bytes) when parsing regtest blocks (which use 48 bytes).
    // Uncomment when lightwalletd properly handles regtest block parsing.
    // rpc_fixture_test!(get_block);
    // rpc_fixture_test!(get_block_nullifiers);
    // rpc_fixture_test!(get_block_range_nullifiers);
    // rpc_fixture_test!(get_block_range_nullifiers_reverse);
    // rpc_fixture_test!(get_block_range_lower);
    // rpc_fixture_test!(get_block_range_upper);
    // rpc_fixture_test!(get_block_range_reverse);
    // rpc_fixture_test!(get_tree_state_by_hash);

    mod get_subtree_roots {
        //! - To run the `get_subtree_roots_sapling` test, sync Zebrad in testnet mode and copy the cache to `zcash_local_net/chain_cache/testnet_get_subtree_roots_sapling`. At least 2 sapling shards must be synced to pass. See [crate::test_fixtures::get_subtree_roots_sapling] doc comments for more details.
        //! - To run the `get_subtree_roots_orchard` test, sync Zebrad in mainnet mode and copy the cache to `zcash_local_net/chain_cache/testnet_get_subtree_roots_orchard`. At least 2 orchard shards must be synced to pass. See [crate::test_fixtures::get_subtree_roots_orchard] doc comments for more details.
        use super::*;
        /// This test requires Zebrad testnet to be already synced to at least 2 sapling shards with the cache at
        /// `zcash_local_net/chain_cache/get_subtree_roots_sapling`
        #[ignore = "this test requires manual setup"]
        #[tokio::test]
        async fn sapling() {
            tracing_subscriber::fmt().init();

            zcash_services_utils::test_fixtures::get_subtree_roots_sapling(
                ZEBRAD_BIN,
                ZAINOD_BIN,
                LIGHTWALLETD_BIN,
                Network::Testnet,
            )
            .await;
        }

        /// This test requires Zebrad mainnet to be already synced to at least 2 sapling shards with the cache at
        /// `zcash_local_net/chain_cache/get_subtree_roots_orchard`
        #[ignore = "this test requires manual setup"]
        #[tokio::test]
        async fn orchard() {
            tracing_subscriber::fmt().init();

            zcash_services_utils::test_fixtures::get_subtree_roots_orchard(
                ZEBRAD_BIN,
                ZAINOD_BIN,
                LIGHTWALLETD_BIN,
                Network::Mainnet,
            )
            .await;
        }
    }
}

//! In practice there are several common scenarios for which helpers are provided.
//! These scenarios vary in the configuration of clients in use.  Most scenarios
//! require some funds, the simplest way to access funds is to use a "faucet".
//! A "faucet" is a client that receives mining rewards (because its spend capability
//! generated the address registered as the `minetoaddress` in the zcash.conf that's
//! used by the 'regetst mode' zcashs backing these tests.).
//! HELPERS:
//! If you just need a faucet, use the "faucet" helper.
//! If you need a faucet, and a single recipient, use 'faucet_recipient`
//! For less common client configurations use the client builder directly with
//! custom_clients
//! All scenarios have a default (i.e. faucet_default) which take minimal parameters and
//! build the scenario with the most common settings. This simplifies test writing in
//! most cases by removing the need for configuration.

const ZCASHD_BIN: Option<PathBuf> = None;
const ZCASH_CLI_BIN: Option<PathBuf> = None;
const ZEBRAD_BIN: Option<PathBuf> = None;
const LIGHTWALLETD_BIN: Option<PathBuf> = None;
const ZAINOD_BIN: Option<PathBuf> = None;

use std::path::PathBuf;

use portpicker::Port;
use testvectors::{
    REG_O_ADDR_FROM_ABANDONART, REG_T_ADDR_FROM_ABANDONART, REG_Z_ADDR_FROM_ABANDONART,
};
use zcash_protocol::{PoolType, ShieldedProtocol};
use zingo_infra_services::LocalNet;
use zingo_infra_services::indexer::{Lightwalletd, LightwalletdConfig};
use zingo_infra_services::network::{ActivationHeights, localhost_uri};
use zingo_infra_services::validator::{Zcashd, ZcashdConfig};

use crate::get_base_address_macro;
use crate::lightclient::LightClient;
use crate::testutils::increase_height_and_wait_for_client;
use crate::testutils::regtest::{ChildProcessHandler, RegtestManager};
use setup::ClientBuilder;
use testvectors::{BASE_HEIGHT, seeds::HOSPITAL_MUSEUM_SEED};

/// TODO: Add Doc Comment Here!
pub mod setup {
    use std::num::NonZeroU32;
    use std::path::PathBuf;

    use bip0039::Mnemonic;
    use pepper_sync::config::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery};
    use tokio::time::sleep;

    use zcash_protocol::{PoolType, ShieldedProtocol};
    use zingo_infra_services::network::ActivationHeights;

    use crate::config::{ChainType, ZingoConfig, load_clientconfig};
    use crate::testutils::RegtestManager;
    use crate::testutils::paths::get_regtest_dir;
    use crate::testutils::regtest::ChildProcessHandler;
    use crate::wallet::keys::unified::ReceiverSelection;
    use crate::wallet::{LightWallet, WalletSettings};
    use crate::{lightclient::LightClient, wallet::WalletBase};
    use testvectors::{
        BASE_HEIGHT, REG_O_ADDR_FROM_ABANDONART, REG_T_ADDR_FROM_ABANDONART,
        REG_Z_ADDR_FROM_ABANDONART, seeds,
    };

    /// TODO: Add Doc Comment Here!
    pub struct ScenarioBuilder {
        /// TODO: Add Doc Comment Here!
        pub test_env: TestEnvironmentGenerator,
        /// TODO: Add Doc Comment Here!
        pub regtest_manager: RegtestManager,
        /// TODO: Add Doc Comment Here!
        pub client_builder: ClientBuilder,
        /// TODO: Add Doc Comment Here!
        pub child_process_handler: Option<ChildProcessHandler>,
    }
    impl ScenarioBuilder {
        fn build_scenario(
            custom_client_config: Option<PathBuf>,
            set_lightwalletd_port: Option<portpicker::Port>,
        ) -> Self {
            //! TestEnvironmentGenerator sets particular parameters, specific filenames,
            //! port numbers, etc.  in general no test_config should be used for
            //! more than one test, and usually is only invoked via this
            //! ScenarioBuilder::new constructor.  If you need to set some value
            //! once, per test, consider adding environment config (e.g. ports, OS) to
            //! TestEnvironmentGenerator and for scenario specific add to this constructor
            let test_env = TestEnvironmentGenerator::new(set_lightwalletd_port);
            let regtest_manager = test_env.regtest_manager.clone();
            let data_dir = if let Some(data_dir) = custom_client_config {
                data_dir
            } else {
                regtest_manager.zingo_datadir.clone()
            };
            let client_builder = ClientBuilder::new(test_env.get_lightwalletd_uri(), data_dir);
            let child_process_handler = None;
            Self {
                test_env,
                regtest_manager,
                client_builder,
                child_process_handler,
            }
        }

        /// TODO: Add Doc Comment Here!
        pub async fn new_load_1153_saplingcb_regtest_chain(
            activation_heights: ActivationHeights,
        ) -> Self {
            // let mut sb = ScenarioBuilder::build_scenario(None, None);
            // let source = get_regtest_dir().join("data/chain_cache/blocks_1153/zcashd/regtest");
            // if !source.exists() {
            //     panic!("Data cache is missing!");
            // }
            // let destination = &sb.regtest_manager.zcashd_data_dir;

            // std::process::Command::new("cp")
            //     .arg("-r")
            //     .arg(source)
            //     .arg(destination)
            //     .output()
            //     .expect("copy operation into fresh dir from known dir to succeed");
            // dbg!(&sb.test_env.regtest_manager.zcashd_config);
            // sb.configure_scenario(
            //     Some(PoolType::Shielded(ShieldedProtocol::Sapling)),
            //     activation_heights,
            //     false,
            // );
            // sb.launch_scenario(false).await;
            // sb
            todo!()
        }
    }

    /// Struct for building lightclients for integration testing
    pub struct ClientBuilder {
        /// Indexer URI
        pub server_id: http::Uri,
        /// Directory for wallet files
        pub zingo_datadir: PathBuf,
        client_number: u8,
    }

    impl ClientBuilder {
        /// TODO: Add Doc Comment Here!
        pub fn new(server_id: http::Uri, zingo_datadir: PathBuf) -> Self {
            let client_number = 0;
            ClientBuilder {
                server_id,
                zingo_datadir,
                client_number,
            }
        }

        pub fn make_unique_data_dir_and_load_config(
            &mut self,
            activation_heights: ActivationHeights,
        ) -> ZingoConfig {
            //! Each client requires a unique data_dir, we use the
            //! client_number counter for this.
            self.client_number += 1;
            let conf_path = format!(
                "{}_client_{}",
                self.zingo_datadir.to_string_lossy(),
                self.client_number
            );
            self.create_clientconfig(PathBuf::from(conf_path), activation_heights)
        }

        /// TODO: Add Doc Comment Here!
        pub fn create_clientconfig(
            &self,
            conf_path: PathBuf,
            activation_heights: ActivationHeights,
        ) -> ZingoConfig {
            std::fs::create_dir(&conf_path).unwrap();
            load_clientconfig(
                self.server_id.clone(),
                Some(conf_path),
                ChainType::Regtest(activation_heights),
                WalletSettings {
                    sync_config: SyncConfig {
                        transparent_address_discovery: TransparentAddressDiscovery::minimal(),
                        performance_level: PerformanceLevel::High,
                    },
                    min_confirmations: NonZeroU32::try_from(1).unwrap(),
                },
                1.try_into().unwrap(),
            )
            .unwrap()
        }

        /// TODO: Add Doc Comment Here!
        pub fn build_faucet(
            &mut self,
            overwrite: bool,
            activation_heights: ActivationHeights,
        ) -> LightClient {
            //! A "faucet" is a lightclient that receives mining rewards
            self.build_client(
                seeds::ABANDON_ART_SEED.to_string(),
                0,
                overwrite,
                activation_heights,
            )
        }

        /// TODO: Add Doc Comment Here!
        pub fn build_client(
            &mut self,
            mnemonic_phrase: String,
            birthday: u64,
            overwrite: bool,
            activation_heights: ActivationHeights,
        ) -> LightClient {
            let config = self.make_unique_data_dir_and_load_config(activation_heights);
            let mut wallet = LightWallet::new(
                config.chain,
                WalletBase::Mnemonic {
                    mnemonic: Mnemonic::from_phrase(mnemonic_phrase).unwrap(),
                    no_of_accounts: 1.try_into().unwrap(),
                },
                (birthday as u32).into(),
                config.wallet_settings.clone(),
            )
            .unwrap();
            wallet
                .generate_unified_address(ReceiverSelection::sapling_only(), zip32::AccountId::ZERO)
                .unwrap();

            LightClient::create_from_wallet(wallet, config, overwrite).unwrap()
        }
    }

    /// TODO: Add Doc Comment Here!
    pub struct TestEnvironmentGenerator {
        zcashd_rpcservice_port: String,
        lightwalletd_rpcservice_port: String,
        regtest_manager: RegtestManager,
        lightwalletd_uri: http::Uri,
    }

    impl TestEnvironmentGenerator {
        /// TODO: Add Doc Comment Here!
        pub(crate) fn new(set_lightwalletd_port: Option<portpicker::Port>) -> Self {
            let zcashd_rpcservice_port = TestEnvironmentGenerator::pick_unused_port_to_string(None);
            let lightwalletd_rpcservice_port =
                TestEnvironmentGenerator::pick_unused_port_to_string(set_lightwalletd_port);
            let regtest_manager = RegtestManager::new(tempfile::TempDir::new().unwrap().keep());
            let server_uri = crate::config::construct_lightwalletd_uri(Some(format!(
                "http://127.0.0.1:{lightwalletd_rpcservice_port}"
            )));
            Self {
                zcashd_rpcservice_port,
                lightwalletd_rpcservice_port,
                regtest_manager,
                lightwalletd_uri: server_uri,
            }
        }

        fn write_contents_and_return_path(&self, configtype: &str, contents: String) -> PathBuf {
            let loc = match configtype {
                "zcash" => &self.regtest_manager.zcashd_config,
                "lightwalletd" => &self.regtest_manager.lightwalletd_config,
                _ => panic!("Unepexted configtype!"),
            };
            let mut output = std::fs::File::create(loc).expect("How could path be missing?");
            std::io::Write::write(&mut output, contents.as_bytes())
                .unwrap_or_else(|_| panic!("Couldn't write {contents}!"));
            loc.clone()
        }

        /// TODO: Add Doc Comment Here!
        pub(crate) fn get_lightwalletd_uri(&self) -> http::Uri {
            self.lightwalletd_uri.clone()
        }

        /// TODO: Add Doc Comment Here!
        pub fn pick_unused_port_to_string(set_port: Option<portpicker::Port>) -> String {
            if let Some(port) = set_port {
                if !portpicker::is_free(port) {
                    panic!("Port is not free!");
                };
                port.to_string()
            } else {
                portpicker::pick_unused_port()
                    .expect("Port unpickable!")
                    .to_string()
            }
        }
    }
}

/// Builds faucet (miner) and recipient lightclients for local network integration testing
pub fn build_lightclients(
    lightclient_dir: PathBuf,
    indexer_port: Port,
) -> (LightClient, LightClient) {
    let mut client_builder = ClientBuilder::new(localhost_uri(indexer_port), lightclient_dir);
    let faucet = client_builder.build_faucet(true, ActivationHeights::default());
    let recipient = client_builder.build_client(
        HOSPITAL_MUSEUM_SEED.to_string(),
        1,
        true,
        ActivationHeights::default(),
    );

    (faucet, recipient)
}

/// TODO: Add Doc Comment Here!
pub async fn unfunded_client(
    activation_heights: ActivationHeights,
    lightwalletd_feature: bool,
) -> (RegtestManager, ChildProcessHandler, LightClient) {
    // let mut scenario_builder = setup::ScenarioBuilder::build_configure_launch(
    //     None,
    //     None,
    //     None,
    //     &activation_heights,
    //     lightwalletd_feature,
    // )
    // .await;
    // (
    //     scenario_builder.regtest_manager,
    //     scenario_builder.child_process_handler.unwrap(),
    //     scenario_builder.client_builder.build_client(
    //         HOSPITAL_MUSEUM_SEED.to_string(),
    //         0,
    //         false,
    //         activation_heights,
    //     ),
    // )
    //
    todo!()
}

/// TODO: Add Doc Comment Here!
pub async fn unfunded_client_default() -> (RegtestManager, ChildProcessHandler, LightClient) {
    let activation_heights = ActivationHeights::default();
    unfunded_client(activation_heights, true).await
}

/// Many scenarios need to start with spendable funds.  This setup provides
/// 3 blocks worth of coinbase to a preregistered spend capability.
///
/// This key is registered to receive block rewards by corresponding to the
/// address registered as the "mineraddress" field in zcash.conf
///
/// The general scenario framework requires instances of zingo-cli, lightwalletd,
/// and zcashd (in regtest mode). This setup is intended to produce the most basic
/// of scenarios.  As scenarios with even less requirements
/// become interesting (e.g. without experimental features, or txindices) we'll create more setups.
pub async fn faucet(
    mine_to_pool: PoolType,
    activation_heights: ActivationHeights,
    lightwalletd_feature: bool,
) -> (RegtestManager, ChildProcessHandler, LightClient) {
    // let mut sb = setup::ScenarioBuilder::build_configure_launch(
    //     Some(mine_to_pool),
    //     None,
    //     None,
    //     &regtest_network,
    //     lightwalletd_feature,
    // )
    // .await;
    // let mut faucet = sb.client_builder.build_faucet(false, regtest_network);
    // faucet.sync_and_await().await.unwrap();
    // (
    //     sb.regtest_manager,
    //     sb.child_process_handler.unwrap(),
    //     faucet,
    // )
    todo!()
}

/// TODO: Add Doc Comment Here!
pub async fn faucet_default() -> (RegtestManager, ChildProcessHandler, LightClient) {
    let activation_heights = ActivationHeights::default();
    faucet(
        PoolType::Shielded(ShieldedProtocol::Orchard),
        activation_heights,
        true,
    )
    .await
}

/// TODO: Add Doc Comment Here!
pub async fn faucet_recipient(
    mine_to_pool: PoolType,
    activation_heights: ActivationHeights,
) -> (LocalNet<Lightwalletd, Zcashd>, LightClient, LightClient) {
    let miner_address = match mine_to_pool {
        PoolType::ORCHARD => REG_O_ADDR_FROM_ABANDONART,
        PoolType::SAPLING => REG_Z_ADDR_FROM_ABANDONART,
        PoolType::Transparent => REG_T_ADDR_FROM_ABANDONART,
    };
    let local_net = LocalNet::<Lightwalletd, Zcashd>::launch(
        LightwalletdConfig {
            lightwalletd_bin: LIGHTWALLETD_BIN,
            listen_port: None,
            zcashd_conf: PathBuf::new(),
        },
        ZcashdConfig {
            zcashd_bin: ZCASHD_BIN,
            zcash_cli_bin: ZCASH_CLI_BIN,
            rpc_listen_port: None,
            activation_heights,
            miner_address: Some(miner_address),
            chain_cache: None,
        },
    )
    .await;

    let lightclient_dir = tempfile::tempdir().unwrap();
    let (mut faucet, mut recipient) = build_lightclients(
        lightclient_dir.path().to_path_buf(),
        local_net.indexer().port(),
    );
    faucet.sync_and_await().await.unwrap();
    recipient.sync_and_await().await.unwrap();

    (local_net, faucet, recipient)
}

/// TODO: Add Doc Comment Here!
pub async fn faucet_recipient_default() -> (LocalNet<Lightwalletd, Zcashd>, LightClient, LightClient)
{
    let activation_heights = ActivationHeights::default();
    faucet_recipient(
        PoolType::Shielded(ShieldedProtocol::Orchard),
        activation_heights,
    )
    .await
}

/// TODO: Add Doc Comment Here!
pub async fn faucet_funded_recipient(
    orchard_funds: Option<u64>,
    sapling_funds: Option<u64>,
    transparent_funds: Option<u64>,
    mine_to_pool: PoolType,
    activation_heights: ActivationHeights,
) -> (
    LocalNet<Lightwalletd, Zcashd>,
    LightClient,
    LightClient,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let (local_net, mut faucet, mut recipient) =
        faucet_recipient(mine_to_pool, activation_heights).await;
    increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
        .await
        .unwrap();

    let orchard_txid = if let Some(funds) = orchard_funds {
        Some(
            super::lightclient::from_inputs::quick_send(
                &mut faucet,
                vec![(&get_base_address_macro!(recipient, "unified"), funds, None)],
            )
            .await
            .unwrap()
            .first()
            .to_string(),
        )
    } else {
        None
    };
    let sapling_txid = if let Some(funds) = sapling_funds {
        Some(
            super::lightclient::from_inputs::quick_send(
                &mut faucet,
                vec![(&get_base_address_macro!(recipient, "sapling"), funds, None)],
            )
            .await
            .unwrap()
            .first()
            .to_string(),
        )
    } else {
        None
    };
    let transparent_txid = if let Some(funds) = transparent_funds {
        Some(
            super::lightclient::from_inputs::quick_send(
                &mut faucet,
                vec![(
                    &get_base_address_macro!(recipient, "transparent"),
                    funds,
                    None,
                )],
            )
            .await
            .unwrap()
            .first()
            .to_string(),
        )
    } else {
        None
    };
    increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
        .await
        .unwrap();
    faucet.sync_and_await().await.unwrap();
    (
        local_net,
        faucet,
        recipient,
        orchard_txid,
        sapling_txid,
        transparent_txid,
    )
}

/// TODO: Add Doc Comment Here!
pub async fn faucet_funded_recipient_default(
    orchard_funds: u64,
) -> (
    LocalNet<Lightwalletd, Zcashd>,
    LightClient,
    LightClient,
    String,
) {
    let activation_heights = ActivationHeights::default();
    let (local_net, faucet, recipient, orchard_txid, _sapling_txid, _transparent_txid) =
        faucet_funded_recipient(
            Some(orchard_funds),
            None,
            None,
            PoolType::Shielded(ShieldedProtocol::Orchard),
            activation_heights,
        )
        .await;
    (local_net, faucet, recipient, orchard_txid.unwrap())
}

/// TODO: Add Doc Comment Here!
pub async fn custom_clients(
    mine_to_pool: PoolType,
    activation_heights: ActivationHeights,
) -> (LocalNet<Lightwalletd, Zcashd>, ClientBuilder) {
    let miner_address = match mine_to_pool {
        PoolType::ORCHARD => REG_O_ADDR_FROM_ABANDONART,
        PoolType::SAPLING => REG_Z_ADDR_FROM_ABANDONART,
        PoolType::Transparent => REG_T_ADDR_FROM_ABANDONART,
    };
    let local_net = LocalNet::<Lightwalletd, Zcashd>::launch(
        LightwalletdConfig {
            lightwalletd_bin: LIGHTWALLETD_BIN,
            listen_port: None,
            zcashd_conf: PathBuf::new(),
        },
        ZcashdConfig {
            zcashd_bin: ZCASHD_BIN,
            zcash_cli_bin: ZCASH_CLI_BIN,
            rpc_listen_port: None,
            activation_heights,
            miner_address: Some(miner_address),
            chain_cache: None,
        },
    )
    .await;
    let client_builder = ClientBuilder::new(
        localhost_uri(local_net.indexer().port()),
        tempfile::tempdir().unwrap().path().to_path_buf(),
    );

    (local_net, client_builder)
}

/// TODO: Add Doc Comment Here!
pub async fn custom_clients_default() -> (
    LocalNet<Lightwalletd, Zcashd>,
    ClientBuilder,
    ActivationHeights,
) {
    let activation_heights = ActivationHeights::default();
    let (local_net, client_builder) = custom_clients(
        PoolType::Shielded(ShieldedProtocol::Orchard),
        activation_heights,
    )
    .await;

    (local_net, client_builder, activation_heights)
}

/// TODO: Add Doc Comment Here!
pub async fn unfunded_mobileclient() -> (RegtestManager, ChildProcessHandler) {
    let activation_heights = ActivationHeights::default();
    // let scenario_builder = setup::ScenarioBuilder::build_configure_launch(
    //     None,
    //     None,
    //     Some(20_000),
    //     activation_heights,
    //     true,
    // )
    // .await;
    // (
    //     scenario_builder.regtest_manager,
    //     scenario_builder.child_process_handler.unwrap(),
    // )
    todo!()
}

/// TODO: Add Doc Comment Here!
pub async fn funded_orchard_mobileclient(value: u64) -> (RegtestManager, ChildProcessHandler) {
    let activation_heights = ActivationHeights::default();
    // let mut scenario_builder = setup::ScenarioBuilder::build_configure_launch(
    //     Some(PoolType::Shielded(ShieldedProtocol::Sapling)),
    //     None,
    //     Some(20_000),
    //     activation_heights,
    //     true,
    // )
    // .await;
    // let mut faucet = scenario_builder
    //     .client_builder
    //     .build_faucet(false, activation_heights);
    // let recipient = scenario_builder.client_builder.build_client(
    //     HOSPITAL_MUSEUM_SEED.to_string(),
    //     0,
    //     false,
    //     activation_heights,
    // );
    // faucet.sync_and_await().await.unwrap();
    // super::lightclient::from_inputs::quick_send(
    //     &mut faucet,
    //     vec![(&get_base_address_macro!(recipient, "unified"), value, None)],
    // )
    // .await
    // .unwrap();
    // scenario_builder
    //     .regtest_manager
    //     .generate_n_blocks(1)
    //     .expect("Failed to generate blocks.");
    // (
    //     scenario_builder.regtest_manager,
    //     scenario_builder.child_process_handler.unwrap(),
    // )
    todo!()
}

/// TODO: Add Doc Comment Here!
pub async fn funded_orchard_with_3_txs_mobileclient(
    value: u64,
) -> (RegtestManager, ChildProcessHandler) {
    let activation_heights = ActivationHeights::default();
    // let mut scenario_builder = setup::ScenarioBuilder::build_configure_launch(
    //     Some(PoolType::Shielded(ShieldedProtocol::Sapling)),
    //     None,
    //     Some(20_000),
    //     activation_heights,
    //     true,
    // )
    // .await;
    // let mut faucet = scenario_builder
    //     .client_builder
    //     .build_faucet(false, activation_heights);
    // let mut recipient = scenario_builder.client_builder.build_client(
    //     HOSPITAL_MUSEUM_SEED.to_string(),
    //     0,
    //     false,
    //     activation_heights,
    // );
    // increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &mut faucet, 1)
    //     .await
    //     .unwrap();
    // // received from a faucet
    // super::lightclient::from_inputs::quick_send(
    //     &mut faucet,
    //     vec![(&get_base_address_macro!(recipient, "unified"), value, None)],
    // )
    // .await
    // .unwrap();
    // increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &mut recipient, 1)
    //     .await
    //     .unwrap();
    // // send to a faucet
    // super::lightclient::from_inputs::quick_send(
    //     &mut recipient,
    //     vec![(
    //         &get_base_address_macro!(faucet, "unified"),
    //         value.checked_div(10).unwrap(),
    //         None,
    //     )],
    // )
    // .await
    // .unwrap();
    // increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &mut recipient, 1)
    //     .await
    //     .unwrap();
    // // send to self sapling
    // let recipient_sapling_address = get_base_address_macro!(recipient, "sapling");
    // super::lightclient::from_inputs::quick_send(
    //     &mut recipient,
    //     vec![(
    //         &recipient_sapling_address,
    //         value.checked_div(10).unwrap(),
    //         Some("note-to-self test memo"),
    //     )],
    // )
    // .await
    // .unwrap();
    // scenario_builder
    //     .regtest_manager
    //     .generate_n_blocks(4)
    //     .expect("Failed to generate blocks.");
    // (
    //     scenario_builder.regtest_manager,
    //     scenario_builder.child_process_handler.unwrap(),
    // )
    todo!()
}

/// This scenario funds a client with transparent funds.
pub async fn funded_transparent_mobileclient(value: u64) -> (RegtestManager, ChildProcessHandler) {
    let activation_heights = ActivationHeights::default();
    // let mut scenario_builder = setup::ScenarioBuilder::build_configure_launch(
    //     Some(PoolType::Shielded(ShieldedProtocol::Sapling)),
    //     None,
    //     Some(20_000),
    //     activation_heights,
    //     true,
    // )
    // .await;
    // let mut faucet = scenario_builder
    //     .client_builder
    //     .build_faucet(false, activation_heights);
    // let mut recipient = scenario_builder.client_builder.build_client(
    //     HOSPITAL_MUSEUM_SEED.to_string(),
    //     0,
    //     false,
    //     activation_heights,
    // );
    // increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &mut faucet, 1)
    //     .await
    //     .unwrap();

    // // received from a faucet to transparent
    // super::lightclient::from_inputs::quick_send(
    //     &mut faucet,
    //     vec![(
    //         &get_base_address_macro!(recipient, "transparent"),
    //         value.checked_div(4).unwrap(),
    //         None,
    //     )],
    // )
    // .await
    // .unwrap();
    // increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &mut recipient, 1)
    //     .await
    //     .unwrap();

    // // end
    // scenario_builder
    //     .regtest_manager
    //     .generate_n_blocks(1)
    //     .expect("Failed to generate blocks.");
    // (
    //     scenario_builder.regtest_manager,
    //     scenario_builder.child_process_handler.unwrap(),
    // )
    todo!()
}

/// TODO: Add Doc Comment Here!
pub async fn funded_orchard_sapling_transparent_shielded_mobileclient(
    value: u64,
) -> (RegtestManager, ChildProcessHandler) {
    let activation_heights = ActivationHeights::default();
    // let mut scenario_builder = setup::ScenarioBuilder::build_configure_launch(
    //     Some(PoolType::Shielded(ShieldedProtocol::Sapling)),
    //     None,
    //     Some(20_000),
    //     activation_heights,
    //     true,
    // )
    // .await;
    // let mut faucet = scenario_builder
    //     .client_builder
    //     .build_faucet(false, activation_heights);
    // let mut recipient = scenario_builder.client_builder.build_client(
    //     HOSPITAL_MUSEUM_SEED.to_string(),
    //     0,
    //     false,
    //     activation_heights,
    // );
    // increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &mut faucet, 1)
    //     .await
    //     .unwrap();
    // // received from a faucet to orchard
    // super::lightclient::from_inputs::quick_send(
    //     &mut faucet,
    //     vec![(
    //         &get_base_address_macro!(recipient, "unified"),
    //         value.checked_div(2).unwrap(),
    //         None,
    //     )],
    // )
    // .await
    // .unwrap();
    // increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &mut faucet, 1)
    //     .await
    //     .unwrap();
    // // received from a faucet to sapling
    // super::lightclient::from_inputs::quick_send(
    //     &mut faucet,
    //     vec![(
    //         &get_base_address_macro!(recipient, "sapling"),
    //         value.checked_div(4).unwrap(),
    //         None,
    //     )],
    // )
    // .await
    // .unwrap();
    // increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &mut faucet, 1)
    //     .await
    //     .unwrap();
    // // received from a faucet to transparent
    // super::lightclient::from_inputs::quick_send(
    //     &mut faucet,
    //     vec![(
    //         &get_base_address_macro!(recipient, "transparent"),
    //         value.checked_div(4).unwrap(),
    //         None,
    //     )],
    // )
    // .await
    // .unwrap();
    // increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &mut recipient, 1)
    //     .await
    //     .unwrap();
    // // send to a faucet
    // super::lightclient::from_inputs::quick_send(
    //     &mut recipient,
    //     vec![(
    //         &get_base_address_macro!(faucet, "unified"),
    //         value.checked_div(10).unwrap(),
    //         None,
    //     )],
    // )
    // .await
    // .unwrap();
    // increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &mut recipient, 1)
    //     .await
    //     .unwrap();
    // // send to self orchard
    // let recipient_unified_address = get_base_address_macro!(recipient, "unified");
    // super::lightclient::from_inputs::quick_send(
    //     &mut recipient,
    //     vec![(
    //         &recipient_unified_address,
    //         value.checked_div(10).unwrap(),
    //         None,
    //     )],
    // )
    // .await
    // .unwrap();
    // increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &mut recipient, 1)
    //     .await
    //     .unwrap();
    // // send to self sapling
    // let recipient_sapling_address = get_base_address_macro!(recipient, "sapling");
    // super::lightclient::from_inputs::quick_send(
    //     &mut recipient,
    //     vec![(
    //         &recipient_sapling_address,
    //         value.checked_div(10).unwrap(),
    //         None,
    //     )],
    // )
    // .await
    // .unwrap();
    // increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &mut recipient, 1)
    //     .await
    //     .unwrap();
    // // send to self transparent
    // let recipient_transparent_address = get_base_address_macro!(recipient, "transparent");
    // super::lightclient::from_inputs::quick_send(
    //     &mut recipient,
    //     vec![(
    //         &recipient_transparent_address,
    //         value.checked_div(10).unwrap(),
    //         None,
    //     )],
    // )
    // .await
    // .unwrap();
    // increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &mut recipient, 1)
    //     .await
    //     .unwrap();
    // // shield transparent
    // recipient
    //     .quick_shield(zip32::AccountId::ZERO)
    //     .await
    //     .unwrap();
    // increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &mut recipient, 1)
    //     .await
    //     .unwrap();
    // // end
    // scenario_builder
    //     .regtest_manager
    //     .generate_n_blocks(1)
    //     .expect("Failed to generate blocks.");
    // (
    //     scenario_builder.regtest_manager,
    //     scenario_builder.child_process_handler.unwrap(),
    // )
    todo!()
}

/// TODO: Add Doc Comment Here!
pub mod chainload {
    use super::*;

    /// TODO: Add Doc Comment Here!
    pub async fn unsynced_basic() -> ChildProcessHandler {
        let activation_heights = ActivationHeights::default();
        setup::ScenarioBuilder::new_load_1153_saplingcb_regtest_chain(activation_heights)
            .await
            .child_process_handler
            .unwrap()
    }

    /// TODO: Add Doc Comment Here!
    pub async fn faucet_recipient_1153() -> (
        RegtestManager,
        ChildProcessHandler,
        LightClient,
        LightClient,
    ) {
        let activation_heights = ActivationHeights::default();
        let mut sb =
            setup::ScenarioBuilder::new_load_1153_saplingcb_regtest_chain(activation_heights).await;
        let mut faucet = sb.client_builder.build_faucet(false, activation_heights);
        faucet.sync_and_await().await.unwrap();
        let recipient = sb.client_builder.build_client(
            HOSPITAL_MUSEUM_SEED.to_string(),
            0,
            false,
            activation_heights,
        );
        (
            sb.regtest_manager,
            sb.child_process_handler.unwrap(),
            faucet,
            recipient,
        )
    }

    /// TODO: Add Doc Comment Here!
    pub async fn unsynced_faucet_recipient_1153() -> (
        RegtestManager,
        ChildProcessHandler,
        LightClient,
        LightClient,
    ) {
        let activation_heights = ActivationHeights::default();
        let mut sb =
            setup::ScenarioBuilder::new_load_1153_saplingcb_regtest_chain(activation_heights).await;
        let faucet = sb.client_builder.build_faucet(false, activation_heights);
        let recipient = sb.client_builder.build_client(
            HOSPITAL_MUSEUM_SEED.to_string(),
            0,
            false,
            activation_heights,
        );
        (
            sb.regtest_manager,
            sb.child_process_handler.unwrap(),
            faucet,
            recipient,
        )
    }
}

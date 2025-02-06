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
use crate::get_base_address_macro;
use crate::lightclient::LightClient;
use crate::testutils::increase_height_and_wait_for_client;
use crate::testutils::regtest::{ChildProcessHandler, RegtestManager};
use setup::ClientBuilder;
use testvectors::{seeds::HOSPITAL_MUSEUM_SEED, BASE_HEIGHT};
use zcash_client_backend::{PoolType, ShieldedProtocol};

mod config_templaters;

/// TODO: Add Doc Comment Here!
pub mod setup {
    use testvectors::{
        seeds, BASE_HEIGHT, REG_O_ADDR_FROM_ABANDONART, REG_T_ADDR_FROM_ABANDONART,
        REG_Z_ADDR_FROM_ABANDONART,
    };
    use zcash_client_backend::{PoolType, ShieldedProtocol};

    use crate::testutils::paths::get_regtest_dir;
    use crate::testutils::poll_server_height;
    use crate::testutils::regtest::ChildProcessHandler;
    use crate::testutils::RegtestManager;
    use crate::{lightclient::LightClient, wallet::WalletBase};
    use std::path::PathBuf;
    use tokio::time::sleep;

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

        fn configure_scenario(
            &mut self,
            mine_to_pool: Option<PoolType>,
            regtest_network: &crate::config::RegtestNetwork,
            lightwalletd_feature: bool,
        ) {
            let mine_to_address = match mine_to_pool {
                Some(PoolType::Shielded(ShieldedProtocol::Orchard)) => {
                    Some(REG_O_ADDR_FROM_ABANDONART)
                }
                Some(PoolType::Shielded(ShieldedProtocol::Sapling)) => {
                    Some(REG_Z_ADDR_FROM_ABANDONART)
                }
                Some(PoolType::Transparent) => Some(REG_T_ADDR_FROM_ABANDONART),
                None => None,
            };
            self.test_env
                .create_zcash_conf(mine_to_address, regtest_network, lightwalletd_feature);
            self.test_env.create_lightwalletd_conf();
        }

        async fn launch_scenario(&mut self, clean: bool) {
            self.child_process_handler = Some(self.regtest_manager.launch(clean).unwrap_or_else(
                |e| match e {
                    super::super::regtest::LaunchChildProcessError::ZcashdState {
                        errorcode,
                        stdout,
                        stderr,
                    } => {
                        panic!("{} {} {}", errorcode, stdout, stderr)
                    }
                },
            ));
            self.regtest_manager
                .generate_n_blocks(BASE_HEIGHT - 1)
                .unwrap();
            while poll_server_height(&self.regtest_manager).as_u32().unwrap() < BASE_HEIGHT {
                sleep(std::time::Duration::from_millis(50)).await;
            }
        }

        /// TODO: Add Doc Comment Here!
        pub async fn new_load_1153_saplingcb_regtest_chain(
            regtest_network: &crate::config::RegtestNetwork,
        ) -> Self {
            let mut sb = ScenarioBuilder::build_scenario(None, None);
            let source = get_regtest_dir().join("data/chain_cache/blocks_1153/zcashd/regtest");
            if !source.exists() {
                panic!("Data cache is missing!");
            }
            let destination = &sb.regtest_manager.zcashd_data_dir;

            std::process::Command::new("cp")
                .arg("-r")
                .arg(source)
                .arg(destination)
                .output()
                .expect("copy operation into fresh dir from known dir to succeed");
            dbg!(&sb.test_env.regtest_manager.zcashd_config);
            sb.configure_scenario(
                Some(PoolType::Shielded(ShieldedProtocol::Sapling)),
                regtest_network,
                false,
            );
            sb.launch_scenario(false).await;
            sb
        }

        /// Writes the specified zcashd.conf and launches with it
        pub async fn build_configure_launch(
            mine_to_pool: Option<PoolType>,
            zingo_wallet_dir: Option<PathBuf>,
            set_lightwalletd_port: Option<portpicker::Port>,
            regtest_network: &crate::config::RegtestNetwork,
            lightwalletd_feature: bool,
        ) -> Self {
            let mut sb = if let Some(conf) = zingo_wallet_dir {
                ScenarioBuilder::build_scenario(Some(conf), set_lightwalletd_port)
            } else {
                ScenarioBuilder::build_scenario(None, set_lightwalletd_port)
            };
            sb.configure_scenario(mine_to_pool, regtest_network, lightwalletd_feature);
            sb.launch_scenario(true).await;
            sb
        }
    }

    /// Internally (and perhaps in wider scopes) we say "Sprout" to mean
    /// take a seed, and generate a client from the seed (planted in the chain).
    pub struct ClientBuilder {
        /// TODO: Add Doc Comment Here!
        pub server_id: http::Uri,
        /// TODO: Add Doc Comment Here!
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
            regtest_network: crate::config::RegtestNetwork,
        ) -> crate::config::ZingoConfig {
            //! Each client requires a unique data_dir, we use the
            //! client_number counter for this.
            self.client_number += 1;
            let conf_path = format!(
                "{}_client_{}",
                self.zingo_datadir.to_string_lossy(),
                self.client_number
            );
            self.create_clientconfig(PathBuf::from(conf_path), regtest_network)
        }

        /// TODO: Add Doc Comment Here!
        pub fn create_clientconfig(
            &self,
            conf_path: PathBuf,
            regtest_network: crate::config::RegtestNetwork,
        ) -> crate::config::ZingoConfig {
            std::fs::create_dir(&conf_path).unwrap();
            crate::config::load_clientconfig(
                self.server_id.clone(),
                Some(conf_path),
                crate::config::ChainType::Regtest(regtest_network),
                true,
            )
            .unwrap()
        }

        /// TODO: Add Doc Comment Here!
        pub async fn build_faucet(
            &mut self,
            overwrite: bool,
            regtest_network: crate::config::RegtestNetwork,
        ) -> LightClient {
            //! A "faucet" is a lightclient that receives mining rewards
            self.build_client(
                seeds::ABANDON_ART_SEED.to_string(),
                0,
                overwrite,
                regtest_network,
            )
            .await
        }

        /// TODO: Add Doc Comment Here!
        pub async fn build_client(
            &mut self,
            mnemonic_phrase: String,
            birthday: u64,
            overwrite: bool,
            regtest_network: crate::config::RegtestNetwork,
        ) -> LightClient {
            let zingo_config = self.make_unique_data_dir_and_load_config(regtest_network);
            LightClient::create_from_wallet_base_async(
                WalletBase::MnemonicPhrase(mnemonic_phrase),
                &zingo_config,
                birthday,
                overwrite,
            )
            .await
            .unwrap()
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
            let regtest_manager =
                RegtestManager::new(tempfile::TempDir::new().unwrap().into_path());
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

        /// TODO: Add Doc Comment Here!
        pub(crate) fn create_zcash_conf(
            &self,
            mine_to_address: Option<&str>,
            regtest_network: &crate::config::RegtestNetwork,
            lightwalletd_feature: bool,
        ) -> PathBuf {
            let config = match mine_to_address {
                Some(address) => super::config_templaters::zcashd::funded(
                    address,
                    &self.zcashd_rpcservice_port,
                    regtest_network,
                    lightwalletd_feature,
                ),
                None => super::config_templaters::zcashd::basic(
                    &self.zcashd_rpcservice_port,
                    regtest_network,
                    lightwalletd_feature,
                    "",
                ),
            };
            self.write_contents_and_return_path("zcash", config)
        }

        /// TODO: Add Doc Comment Here!
        pub(crate) fn create_lightwalletd_conf(&self) -> PathBuf {
            self.write_contents_and_return_path(
                "lightwalletd",
                super::config_templaters::lightwalletd::basic(&self.lightwalletd_rpcservice_port),
            )
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

/// TODO: Add Doc Comment Here!
pub async fn unfunded_client(
    regtest_network: crate::config::RegtestNetwork,
    lightwalletd_feature: bool,
) -> (RegtestManager, ChildProcessHandler, LightClient) {
    let mut scenario_builder = setup::ScenarioBuilder::build_configure_launch(
        None,
        None,
        None,
        &regtest_network,
        lightwalletd_feature,
    )
    .await;
    (
        scenario_builder.regtest_manager,
        scenario_builder.child_process_handler.unwrap(),
        scenario_builder
            .client_builder
            .build_client(HOSPITAL_MUSEUM_SEED.to_string(), 0, false, regtest_network)
            .await,
    )
}

/// TODO: Add Doc Comment Here!
pub async fn unfunded_client_default() -> (RegtestManager, ChildProcessHandler, LightClient) {
    let regtest_network = crate::config::RegtestNetwork::all_upgrades_active();
    unfunded_client(regtest_network, true).await
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
    regtest_network: crate::config::RegtestNetwork,
    lightwalletd_feature: bool,
) -> (RegtestManager, ChildProcessHandler, LightClient) {
    let mut sb = setup::ScenarioBuilder::build_configure_launch(
        Some(mine_to_pool),
        None,
        None,
        &regtest_network,
        lightwalletd_feature,
    )
    .await;
    let faucet = sb.client_builder.build_faucet(false, regtest_network).await;
    faucet.do_sync(false).await.unwrap();
    (
        sb.regtest_manager,
        sb.child_process_handler.unwrap(),
        faucet,
    )
}

/// TODO: Add Doc Comment Here!
pub async fn faucet_default() -> (RegtestManager, ChildProcessHandler, LightClient) {
    let regtest_network = crate::config::RegtestNetwork::all_upgrades_active();
    faucet(
        PoolType::Shielded(ShieldedProtocol::Orchard),
        regtest_network,
        true,
    )
    .await
}

/// TODO: Add Doc Comment Here!
pub async fn faucet_recipient(
    mine_to_pool: PoolType,
    regtest_network: crate::config::RegtestNetwork,
    lightwalletd_feature: bool,
) -> (
    RegtestManager,
    ChildProcessHandler,
    LightClient,
    LightClient,
) {
    let mut sb = setup::ScenarioBuilder::build_configure_launch(
        Some(mine_to_pool),
        None,
        None,
        &regtest_network,
        lightwalletd_feature,
    )
    .await;
    let faucet = sb.client_builder.build_faucet(false, regtest_network).await;
    faucet.do_sync(false).await.unwrap();

    let recipient = sb
        .client_builder
        .build_client(
            HOSPITAL_MUSEUM_SEED.to_string(),
            BASE_HEIGHT as u64,
            false,
            regtest_network,
        )
        .await;
    (
        sb.regtest_manager,
        sb.child_process_handler.unwrap(),
        faucet,
        recipient,
    )
}

/// TODO: Add Doc Comment Here!
pub async fn faucet_recipient_default() -> (
    RegtestManager,
    ChildProcessHandler,
    LightClient,
    LightClient,
) {
    let regtest_network = crate::config::RegtestNetwork::all_upgrades_active();
    faucet_recipient(
        PoolType::Shielded(ShieldedProtocol::Orchard),
        regtest_network,
        true,
    )
    .await
}

/// TODO: Add Doc Comment Here!
pub async fn faucet_funded_recipient(
    orchard_funds: Option<u64>,
    sapling_funds: Option<u64>,
    transparent_funds: Option<u64>,
    mine_to_pool: PoolType,
    regtest_network: crate::config::RegtestNetwork,
    lightwalletd_feature: bool,
) -> (
    RegtestManager,
    ChildProcessHandler,
    LightClient,
    LightClient,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let (regtest_manager, child_process_handler, faucet, recipient) =
        faucet_recipient(mine_to_pool, regtest_network, lightwalletd_feature).await;
    increase_height_and_wait_for_client(&regtest_manager, &faucet, 1)
        .await
        .unwrap();
    let orchard_txid = if let Some(funds) = orchard_funds {
        Some(
            super::lightclient::from_inputs::quick_send(
                &faucet,
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
                &faucet,
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
                &faucet,
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
    increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
        .await
        .unwrap();
    faucet.do_sync(false).await.unwrap();
    (
        regtest_manager,
        child_process_handler,
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
    RegtestManager,
    ChildProcessHandler,
    LightClient,
    LightClient,
    String,
) {
    let regtest_network = crate::config::RegtestNetwork::all_upgrades_active();
    let (regtest_manager, cph, faucet, recipient, orchard_txid, _sapling_txid, _transparent_txid) =
        faucet_funded_recipient(
            Some(orchard_funds),
            None,
            None,
            PoolType::Shielded(ShieldedProtocol::Orchard),
            regtest_network,
            true,
        )
        .await;
    (
        regtest_manager,
        cph,
        faucet,
        recipient,
        orchard_txid.unwrap(),
    )
}

/// TODO: Add Doc Comment Here!
pub async fn custom_clients(
    mine_to_pool: PoolType,
    regtest_network: crate::config::RegtestNetwork,
    lightwalletd_feature: bool,
) -> (RegtestManager, ChildProcessHandler, ClientBuilder) {
    let sb = setup::ScenarioBuilder::build_configure_launch(
        Some(mine_to_pool),
        None,
        None,
        &regtest_network,
        lightwalletd_feature,
    )
    .await;
    (
        sb.regtest_manager,
        sb.child_process_handler.unwrap(),
        sb.client_builder,
    )
}

/// TODO: Add Doc Comment Here!
pub async fn custom_clients_default() -> (
    RegtestManager,
    ChildProcessHandler,
    ClientBuilder,
    crate::config::RegtestNetwork,
) {
    let regtest_network = crate::config::RegtestNetwork::all_upgrades_active();
    let (regtest_manager, cph, client_builder) = custom_clients(
        PoolType::Shielded(ShieldedProtocol::Orchard),
        regtest_network,
        true,
    )
    .await;
    (regtest_manager, cph, client_builder, regtest_network)
}

/// TODO: Add Doc Comment Here!
pub async fn unfunded_mobileclient() -> (RegtestManager, ChildProcessHandler) {
    let regtest_network = crate::config::RegtestNetwork::all_upgrades_active();
    let scenario_builder = setup::ScenarioBuilder::build_configure_launch(
        None,
        None,
        Some(20_000),
        &regtest_network,
        true,
    )
    .await;
    (
        scenario_builder.regtest_manager,
        scenario_builder.child_process_handler.unwrap(),
    )
}

/// TODO: Add Doc Comment Here!
pub async fn funded_orchard_mobileclient(value: u64) -> (RegtestManager, ChildProcessHandler) {
    let regtest_network = crate::config::RegtestNetwork::all_upgrades_active();
    let mut scenario_builder = setup::ScenarioBuilder::build_configure_launch(
        Some(PoolType::Shielded(ShieldedProtocol::Sapling)),
        None,
        Some(20_000),
        &regtest_network,
        true,
    )
    .await;
    let faucet = scenario_builder
        .client_builder
        .build_faucet(false, regtest_network)
        .await;
    let recipient = scenario_builder
        .client_builder
        .build_client(HOSPITAL_MUSEUM_SEED.to_string(), 0, false, regtest_network)
        .await;
    faucet.do_sync(false).await.unwrap();
    super::lightclient::from_inputs::quick_send(
        &faucet,
        vec![(&get_base_address_macro!(recipient, "unified"), value, None)],
    )
    .await
    .unwrap();
    scenario_builder
        .regtest_manager
        .generate_n_blocks(1)
        .expect("Failed to generate blocks.");
    (
        scenario_builder.regtest_manager,
        scenario_builder.child_process_handler.unwrap(),
    )
}

/// TODO: Add Doc Comment Here!
pub async fn funded_orchard_with_3_txs_mobileclient(
    value: u64,
) -> (RegtestManager, ChildProcessHandler) {
    let regtest_network = crate::config::RegtestNetwork::all_upgrades_active();
    let mut scenario_builder = setup::ScenarioBuilder::build_configure_launch(
        Some(PoolType::Shielded(ShieldedProtocol::Sapling)),
        None,
        Some(20_000),
        &regtest_network,
        true,
    )
    .await;
    let faucet = scenario_builder
        .client_builder
        .build_faucet(false, regtest_network)
        .await;
    let recipient = scenario_builder
        .client_builder
        .build_client(HOSPITAL_MUSEUM_SEED.to_string(), 0, false, regtest_network)
        .await;
    increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &faucet, 1)
        .await
        .unwrap();
    // received from a faucet
    super::lightclient::from_inputs::quick_send(
        &faucet,
        vec![(&get_base_address_macro!(recipient, "unified"), value, None)],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &recipient, 1)
        .await
        .unwrap();
    // send to a faucet
    super::lightclient::from_inputs::quick_send(
        &recipient,
        vec![(
            &get_base_address_macro!(faucet, "unified"),
            value.checked_div(10).unwrap(),
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &recipient, 1)
        .await
        .unwrap();
    // send to self sapling
    super::lightclient::from_inputs::quick_send(
        &recipient,
        vec![(
            &get_base_address_macro!(recipient, "sapling"),
            value.checked_div(10).unwrap(),
            Some("note-to-self test memo"),
        )],
    )
    .await
    .unwrap();
    scenario_builder
        .regtest_manager
        .generate_n_blocks(4)
        .expect("Failed to generate blocks.");
    (
        scenario_builder.regtest_manager,
        scenario_builder.child_process_handler.unwrap(),
    )
}

/// This scenario funds a client with transparent funds.
pub async fn funded_transparent_mobileclient(value: u64) -> (RegtestManager, ChildProcessHandler) {
    let regtest_network = crate::config::RegtestNetwork::all_upgrades_active();
    let mut scenario_builder = setup::ScenarioBuilder::build_configure_launch(
        Some(PoolType::Shielded(ShieldedProtocol::Sapling)),
        None,
        Some(20_000),
        &regtest_network,
        true,
    )
    .await;
    let faucet = scenario_builder
        .client_builder
        .build_faucet(false, regtest_network)
        .await;
    let recipient = scenario_builder
        .client_builder
        .build_client(HOSPITAL_MUSEUM_SEED.to_string(), 0, false, regtest_network)
        .await;
    increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &faucet, 1)
        .await
        .unwrap();

    // received from a faucet to transparent
    super::lightclient::from_inputs::quick_send(
        &faucet,
        vec![(
            &get_base_address_macro!(recipient, "transparent"),
            value.checked_div(4).unwrap(),
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &recipient, 1)
        .await
        .unwrap();

    // end
    scenario_builder
        .regtest_manager
        .generate_n_blocks(1)
        .expect("Failed to generate blocks.");
    (
        scenario_builder.regtest_manager,
        scenario_builder.child_process_handler.unwrap(),
    )
}

/// TODO: Add Doc Comment Here!
pub async fn funded_orchard_sapling_transparent_shielded_mobileclient(
    value: u64,
) -> (RegtestManager, ChildProcessHandler) {
    let regtest_network = crate::config::RegtestNetwork::all_upgrades_active();
    let mut scenario_builder = setup::ScenarioBuilder::build_configure_launch(
        Some(PoolType::Shielded(ShieldedProtocol::Sapling)),
        None,
        Some(20_000),
        &regtest_network,
        true,
    )
    .await;
    let faucet = scenario_builder
        .client_builder
        .build_faucet(false, regtest_network)
        .await;
    let recipient = scenario_builder
        .client_builder
        .build_client(HOSPITAL_MUSEUM_SEED.to_string(), 0, false, regtest_network)
        .await;
    increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &faucet, 1)
        .await
        .unwrap();
    // received from a faucet to orchard
    super::lightclient::from_inputs::quick_send(
        &faucet,
        vec![(
            &get_base_address_macro!(recipient, "unified"),
            value.checked_div(2).unwrap(),
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &faucet, 1)
        .await
        .unwrap();
    // received from a faucet to sapling
    super::lightclient::from_inputs::quick_send(
        &faucet,
        vec![(
            &get_base_address_macro!(recipient, "sapling"),
            value.checked_div(4).unwrap(),
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &faucet, 1)
        .await
        .unwrap();
    // received from a faucet to transparent
    super::lightclient::from_inputs::quick_send(
        &faucet,
        vec![(
            &get_base_address_macro!(recipient, "transparent"),
            value.checked_div(4).unwrap(),
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &recipient, 1)
        .await
        .unwrap();
    // send to a faucet
    super::lightclient::from_inputs::quick_send(
        &recipient,
        vec![(
            &get_base_address_macro!(faucet, "unified"),
            value.checked_div(10).unwrap(),
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &recipient, 1)
        .await
        .unwrap();
    // send to self orchard
    super::lightclient::from_inputs::quick_send(
        &recipient,
        vec![(
            &get_base_address_macro!(recipient, "unified"),
            value.checked_div(10).unwrap(),
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &recipient, 1)
        .await
        .unwrap();
    // send to self sapling
    super::lightclient::from_inputs::quick_send(
        &recipient,
        vec![(
            &get_base_address_macro!(recipient, "sapling"),
            value.checked_div(10).unwrap(),
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &recipient, 1)
        .await
        .unwrap();
    // send to self transparent
    super::lightclient::from_inputs::quick_send(
        &recipient,
        vec![(
            &get_base_address_macro!(recipient, "transparent"),
            value.checked_div(10).unwrap(),
            None,
        )],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &recipient, 1)
        .await
        .unwrap();
    // shield transparent
    recipient.quick_shield().await.unwrap();
    increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &recipient, 1)
        .await
        .unwrap();
    // end
    scenario_builder
        .regtest_manager
        .generate_n_blocks(1)
        .expect("Failed to generate blocks.");
    (
        scenario_builder.regtest_manager,
        scenario_builder.child_process_handler.unwrap(),
    )
}

/// TODO: Add Doc Comment Here!
pub mod chainload {
    use super::*;

    /// TODO: Add Doc Comment Here!
    pub async fn unsynced_basic() -> ChildProcessHandler {
        let regtest_network = crate::config::RegtestNetwork::all_upgrades_active();
        setup::ScenarioBuilder::new_load_1153_saplingcb_regtest_chain(&regtest_network)
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
        let regtest_network = crate::config::RegtestNetwork::all_upgrades_active();
        let mut sb =
            setup::ScenarioBuilder::new_load_1153_saplingcb_regtest_chain(&regtest_network).await;
        let faucet = sb.client_builder.build_faucet(false, regtest_network).await;
        faucet.do_sync(false).await.unwrap();
        let recipient = sb
            .client_builder
            .build_client(HOSPITAL_MUSEUM_SEED.to_string(), 0, false, regtest_network)
            .await;
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
        let regtest_network = crate::config::RegtestNetwork::all_upgrades_active();
        let mut sb =
            setup::ScenarioBuilder::new_load_1153_saplingcb_regtest_chain(&regtest_network).await;
        let faucet = sb.client_builder.build_faucet(false, regtest_network).await;
        let recipient = sb
            .client_builder
            .build_client(HOSPITAL_MUSEUM_SEED.to_string(), 0, false, regtest_network)
            .await;
        (
            sb.regtest_manager,
            sb.child_process_handler.unwrap(),
            faucet,
            recipient,
        )
    }
}

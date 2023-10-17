pub mod data;
pub use incrementalmerkletree;
use zcash_address::unified::{Fvk, Ufvk};
use zingolib::wallet::keys::unified::WalletCapability;
use zingolib::wallet::WalletBase;
pub mod regtest;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::string::String;
use std::time::Duration;

use json::JsonValue;
use log::debug;
use regtest::RegtestManager;
use tokio::time::sleep;
use zingoconfig::{ChainType, ZingoConfig};
use zingolib::lightclient::LightClient;

use crate::scenarios::setup::TestEnvironmentGenerator;

pub const BASE_HEIGHT: u32 = 3;

pub fn build_fvks_from_wallet_capability(wallet_capability: &WalletCapability) -> [Fvk; 3] {
    let o_fvk = Fvk::Orchard(
        orchard::keys::FullViewingKey::try_from(wallet_capability)
            .unwrap()
            .to_bytes(),
    );
    let s_fvk = Fvk::Sapling(
        zcash_primitives::zip32::sapling::DiversifiableFullViewingKey::try_from(wallet_capability)
            .unwrap()
            .to_bytes(),
    );
    let mut t_fvk_bytes = [0u8; 65];
    let t_ext_pk: zingolib::wallet::keys::extended_transparent::ExtendedPubKey =
        (wallet_capability).try_into().unwrap();
    t_fvk_bytes[0..32].copy_from_slice(&t_ext_pk.chain_code[..]);
    t_fvk_bytes[32..65].copy_from_slice(&t_ext_pk.public_key.serialize()[..]);
    let t_fvk = Fvk::P2pkh(t_fvk_bytes);
    [o_fvk, s_fvk, t_fvk]
}
pub async fn build_fvk_client(fvks: &[&Fvk], zingoconfig: &ZingoConfig) -> LightClient {
    let ufvk = zcash_address::unified::Encoding::encode(
        &<Ufvk as zcash_address::unified::Encoding>::try_from_items(
            fvks.iter().copied().cloned().collect(),
        )
        .unwrap(),
        &zcash_address::Network::Regtest,
    );
    LightClient::create_unconnected(zingoconfig, WalletBase::Ufvk(ufvk), 0).unwrap()
}

async fn get_synced_wallet_height(client: &LightClient) -> Result<u32, String> {
    client.do_sync(true).await?;
    Ok(client
        .do_wallet_last_scanned_height()
        .await
        .as_u32()
        .unwrap())
}

fn poll_server_height(manager: &RegtestManager) -> JsonValue {
    let temp_tips = manager.get_chain_tip().unwrap().stdout;
    let tips = json::parse(&String::from_utf8_lossy(&temp_tips)).unwrap();
    tips[0]["height"].clone()
}
// This function _DOES NOT SYNC THE CLIENT/WALLET_.
pub async fn increase_server_height(manager: &RegtestManager, n: u32) {
    let start_height = poll_server_height(manager).as_fixed_point_u64(2).unwrap();
    let target = start_height + n as u64;
    manager
        .generate_n_blocks(n)
        .expect("Called for side effect, failed!");
    let mut count = 0;
    while poll_server_height(manager).as_fixed_point_u64(2).unwrap() < target {
        sleep(Duration::from_millis(50)).await;
        count = dbg!(count + 1);
    }
}

/// Transaction creation involves using a nonce, which means a non-deterministic txid.
/// Datetime is also based on time of run.
/// Check all the other fields
pub fn check_transaction_equality(first: &JsonValue, second: &JsonValue) -> bool {
    for (t1, t2) in [(first, second), (second, first)] {
        for (key1, val1) in t1.entries() {
            if key1 == "txid" || key1 == "datetime" {
                continue;
            }
            if !t2
                .entries()
                .any(|(key2, val2)| key1 == key2 && val1 == val2)
            {
                return false;
            }
        }
    }
    true
}

pub async fn send_value_between_clients_and_sync(
    manager: &RegtestManager,
    sender: &LightClient,
    recipient: &LightClient,
    value: u64,
    address_type: &str,
) -> Result<String, String> {
    debug!(
        "recipient address is: {}",
        &recipient.do_addresses().await[0]["address"]
    );
    let txid = sender
        .do_send(vec![(
            &zingolib::get_base_address!(recipient, address_type),
            value,
            None,
        )])
        .await
        .unwrap();
    increase_height_and_wait_for_client(manager, sender, 1).await?;
    recipient.do_sync(false).await?;
    Ok(txid)
}

// This function increases the chain height reliably (with polling) but
// it _also_ ensures that the client state is synced.
// Unsynced clients are very interesting to us.  See increate_server_height
// to reliably increase the server without syncing the client
pub async fn increase_height_and_wait_for_client(
    manager: &RegtestManager,
    client: &LightClient,
    n: u32,
) -> Result<(), String> {
    wait_until_client_reaches_block_height(
        client,
        generate_n_blocks_return_new_height(manager, n)
            .await
            .expect("should find target height"),
    )
    .await
}
pub async fn generate_n_blocks_return_new_height(
    manager: &RegtestManager,
    n: u32,
) -> Result<u32, String> {
    let start_height = manager.get_current_height().unwrap();
    let target = start_height + n;
    manager
        .generate_n_blocks(n)
        .expect("Called for side effect, failed!");
    assert_eq!(manager.get_current_height().unwrap(), target);
    Ok(target)
}
///will hang if RegtestManager does not reach target_block_height
pub async fn wait_until_client_reaches_block_height(
    client: &LightClient,
    target_block_height: u32,
) -> Result<(), String> {
    while check_wallet_chainheight_value(client, target_block_height).await? {
        sleep(Duration::from_millis(50)).await;
    }
    Ok(())
}
async fn check_wallet_chainheight_value(client: &LightClient, target: u32) -> Result<bool, String> {
    Ok(get_synced_wallet_height(client).await? != target)
}

pub fn get_wallet_nym(nym: &str) -> Result<(String, PathBuf, PathBuf), String> {
    match nym {
        "sap_only" | "orch_only" | "orch_and_sapl" | "tadd_only" => {
            let one_sapling_wallet = format!(
                "{}/tests/data/wallets/v26/202302_release/regtest/{nym}/zingo-wallet.dat",
                regtest::get_cargo_manifest_dir().to_string_lossy()
            );
            let wallet_path = Path::new(&one_sapling_wallet);
            let wallet_dir = wallet_path.parent().unwrap();
            Ok((
                one_sapling_wallet.clone(),
                wallet_path.to_path_buf(),
                wallet_dir.to_path_buf(),
            ))
        }
        _ => Err(format!("nym {nym} not a valid wallet directory")),
    }
}

pub struct RecordingReader<Reader> {
    from: Reader,
    read_lengths: Vec<usize>,
}
impl<T> Read for RecordingReader<T>
where
    T: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let for_info = self.from.read(buf)?;
        log::info!("{:?}", for_info);
        self.read_lengths.push(for_info);
        Ok(for_info)
    }
}
pub async fn load_wallet(
    dir: PathBuf,
    chaintype: ChainType,
) -> (zingolib::wallet::LightWallet, ZingoConfig) {
    let wallet = dir.join("zingo-wallet.dat");
    let lightwalletd_uri = TestEnvironmentGenerator::new(None).get_lightwalletd_uri();
    let zingo_config =
        zingolib::load_clientconfig(lightwalletd_uri, Some(dir), chaintype, true).unwrap();
    let from = std::fs::File::open(wallet).unwrap();

    let read_lengths = vec![];
    let mut recording_reader = RecordingReader { from, read_lengths };

    (
        zingolib::wallet::LightWallet::read_internal(&mut recording_reader, &zingo_config)
            .await
            .unwrap(),
        zingo_config,
    )
}
pub mod scenarios {
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
    use self::setup::ClientBuilder;
    use super::regtest::{ChildProcessHandler, RegtestManager};
    use crate::{
        data::{self, seeds::HOSPITAL_MUSEUM_SEED},
        increase_height_and_wait_for_client, BASE_HEIGHT,
    };
    use zingolib::{get_base_address, lightclient::LightClient, wallet::Pool};

    pub mod setup {
        use crate::data::{
            seeds, REG_O_ADDR_FROM_ABANDONART, REG_T_ADDR_FROM_ABANDONART,
            REG_Z_ADDR_FROM_ABANDONART,
        };
        use crate::BASE_HEIGHT;

        use super::super::regtest::get_regtest_dir;
        use super::{data, ChildProcessHandler, RegtestManager};
        use std::path::PathBuf;
        use tokio::time::sleep;
        use zingolib::wallet::Pool;
        use zingolib::{lightclient::LightClient, wallet::WalletBase};
        pub struct ScenarioBuilder {
            pub test_env: TestEnvironmentGenerator,
            pub regtest_manager: RegtestManager,
            pub client_builder: ClientBuilder,
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
                mine_to_pool: Option<Pool>,
                regtest_network: &zingoconfig::RegtestNetwork,
            ) {
                let mine_to_address = match mine_to_pool {
                    Some(Pool::Orchard) => Some(REG_O_ADDR_FROM_ABANDONART),
                    Some(Pool::Sapling) => Some(REG_Z_ADDR_FROM_ABANDONART),
                    Some(Pool::Transparent) => Some(REG_T_ADDR_FROM_ABANDONART),
                    None => None,
                };
                self.test_env
                    .create_zcash_conf(mine_to_address, regtest_network);
                self.test_env.create_lightwalletd_conf();
            }
            async fn launch_scenario(&mut self, clean: bool) {
                self.child_process_handler = Some(
                    self.regtest_manager
                        .launch(clean)
                        .unwrap_or_else(|e| match e {
                            super::super::regtest::LaunchChildProcessError::ZcashdState {
                                errorcode,
                                stdout,
                                stderr,
                            } => {
                                panic!("{} {} {}", errorcode, stdout, stderr)
                            }
                        }),
                );
                self.regtest_manager
                    .generate_n_blocks(BASE_HEIGHT - 1)
                    .unwrap();
                while crate::poll_server_height(&self.regtest_manager) != BASE_HEIGHT {
                    sleep(std::time::Duration::from_millis(50)).await;
                }
            }
            pub async fn new_load_1153_saplingcb_regtest_chain(
                regtest_network: &zingoconfig::RegtestNetwork,
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
                sb.configure_scenario(Some(Pool::Sapling), regtest_network);
                sb.launch_scenario(false).await;
                sb
            }

            /// Writes the specified zcashd.conf and launches with it
            pub async fn build_configure_launch(
                mine_to_pool: Option<Pool>,
                zingo_wallet_dir: Option<PathBuf>,
                set_lightwalletd_port: Option<portpicker::Port>,
                regtest_network: &zingoconfig::RegtestNetwork,
            ) -> Self {
                let mut sb = if let Some(conf) = zingo_wallet_dir {
                    ScenarioBuilder::build_scenario(Some(conf), set_lightwalletd_port)
                } else {
                    ScenarioBuilder::build_scenario(None, set_lightwalletd_port)
                };
                sb.configure_scenario(mine_to_pool, regtest_network);
                sb.launch_scenario(true).await;
                sb
            }
        }

        /// Internally (and perhaps in wider scopes) we say "Sprout" to mean
        /// take a seed, and generate a client from the seed (planted in the chain).
        pub struct ClientBuilder {
            pub server_id: http::Uri,
            pub zingo_datadir: PathBuf,
            client_number: u8,
        }
        impl ClientBuilder {
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
                regtest_network: zingoconfig::RegtestNetwork,
            ) -> zingoconfig::ZingoConfig {
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
            pub fn create_clientconfig(
                &self,
                conf_path: PathBuf,
                regtest_network: zingoconfig::RegtestNetwork,
            ) -> zingoconfig::ZingoConfig {
                std::fs::create_dir(&conf_path).unwrap();
                zingolib::load_clientconfig(
                    self.server_id.clone(),
                    Some(conf_path),
                    zingoconfig::ChainType::Regtest(regtest_network),
                    true,
                )
                .unwrap()
            }

            pub async fn build_faucet(
                &mut self,
                overwrite: bool,
                regtest_network: zingoconfig::RegtestNetwork,
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
            pub async fn build_client(
                &mut self,
                mnemonic_phrase: String,
                birthday: u64,
                overwrite: bool,
                regtest_network: zingoconfig::RegtestNetwork,
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
        pub struct TestEnvironmentGenerator {
            zcashd_rpcservice_port: String,
            lightwalletd_rpcservice_port: String,
            regtest_manager: RegtestManager,
            lightwalletd_uri: http::Uri,
        }
        impl TestEnvironmentGenerator {
            pub(crate) fn new(set_lightwalletd_port: Option<portpicker::Port>) -> Self {
                let zcashd_rpcservice_port = portpicker::pick_unused_port()
                    .expect("Port unpickable!")
                    .to_string();
                let lightwalletd_rpcservice_port =
                    if let Some(lightwalletd_port) = set_lightwalletd_port {
                        if !portpicker::is_free(lightwalletd_port) {
                            panic!("Lightwalletd RPC service port is not free!");
                        };
                        lightwalletd_port.to_string()
                    } else {
                        portpicker::pick_unused_port()
                            .expect("Port unpickable!")
                            .to_string()
                    };
                let regtest_manager = RegtestManager::new(
                    tempdir::TempDir::new("zingo_integration_test")
                        .unwrap()
                        .into_path(),
                );
                let server_uri = zingoconfig::construct_lightwalletd_uri(Some(format!(
                    "http://127.0.0.1:{lightwalletd_rpcservice_port}"
                )));
                Self {
                    zcashd_rpcservice_port,
                    lightwalletd_rpcservice_port,
                    regtest_manager,
                    lightwalletd_uri: server_uri,
                }
            }
            pub(crate) fn create_zcash_conf(
                &self,
                mine_to_address: Option<&str>,
                regtest_network: &zingoconfig::RegtestNetwork,
            ) -> PathBuf {
                let config = match mine_to_address {
                    Some(address) => data::config_template_fillers::zcashd::funded(
                        address,
                        &self.zcashd_rpcservice_port,
                        regtest_network,
                    ),
                    None => data::config_template_fillers::zcashd::basic(
                        &self.zcashd_rpcservice_port,
                        regtest_network,
                        "",
                    ),
                };
                self.write_contents_and_return_path("zcash", config)
            }
            pub(crate) fn create_lightwalletd_conf(&self) -> PathBuf {
                self.write_contents_and_return_path(
                    "lightwalletd",
                    data::config_template_fillers::lightwalletd::basic(
                        &self.lightwalletd_rpcservice_port,
                    ),
                )
            }
            fn write_contents_and_return_path(
                &self,
                configtype: &str,
                contents: String,
            ) -> PathBuf {
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
            pub(crate) fn get_lightwalletd_uri(&self) -> http::Uri {
                self.lightwalletd_uri.clone()
            }
        }
    }

    pub async fn unfunded_client(
        regtest_network: zingoconfig::RegtestNetwork,
    ) -> (RegtestManager, ChildProcessHandler, LightClient) {
        let mut scenario_builder =
            setup::ScenarioBuilder::build_configure_launch(None, None, None, &regtest_network)
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
    pub async fn unfunded_client_default() -> (RegtestManager, ChildProcessHandler, LightClient) {
        let regtest_network = zingoconfig::RegtestNetwork::all_upgrades_active();
        unfunded_client(regtest_network).await
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
        mine_to_pool: Pool,
        regtest_network: zingoconfig::RegtestNetwork,
    ) -> (RegtestManager, ChildProcessHandler, LightClient) {
        let mut sb = setup::ScenarioBuilder::build_configure_launch(
            Some(mine_to_pool),
            None,
            None,
            &regtest_network,
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
    pub async fn faucet_default() -> (RegtestManager, ChildProcessHandler, LightClient) {
        let regtest_network = zingoconfig::RegtestNetwork::all_upgrades_active();
        faucet(Pool::Orchard, regtest_network).await
    }

    pub async fn faucet_recipient(
        mine_to_pool: Pool,
        regtest_network: zingoconfig::RegtestNetwork,
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
    pub async fn faucet_recipient_default() -> (
        RegtestManager,
        ChildProcessHandler,
        LightClient,
        LightClient,
    ) {
        let regtest_network = zingoconfig::RegtestNetwork::all_upgrades_active();
        faucet_recipient(Pool::Orchard, regtest_network).await
    }

    pub async fn faucet_funded_recipient(
        orchard_funds: Option<u64>,
        sapling_funds: Option<u64>,
        transparent_funds: Option<u64>,
        mine_to_pool: Pool,
        regtest_network: zingoconfig::RegtestNetwork,
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
            faucet_recipient(mine_to_pool, regtest_network).await;
        increase_height_and_wait_for_client(&regtest_manager, &faucet, 1)
            .await
            .unwrap();
        let orchard_txid = if let Some(funds) = orchard_funds {
            Some(
                faucet
                    .do_send(vec![(
                        &get_base_address!(recipient, "unified"),
                        funds,
                        None,
                    )])
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        let sapling_txid = if let Some(funds) = sapling_funds {
            Some(
                faucet
                    .do_send(vec![(
                        &get_base_address!(recipient, "sapling"),
                        funds,
                        None,
                    )])
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        let transparent_txid = if let Some(funds) = transparent_funds {
            Some(
                faucet
                    .do_send(vec![(
                        &get_base_address!(recipient, "transparent"),
                        funds,
                        None,
                    )])
                    .await
                    .unwrap(),
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
    pub async fn faucet_funded_recipient_default(
        orchard_funds: u64,
    ) -> (
        RegtestManager,
        ChildProcessHandler,
        LightClient,
        LightClient,
        String,
    ) {
        let regtest_network = zingoconfig::RegtestNetwork::all_upgrades_active();
        let (
            regtest_manager,
            cph,
            faucet,
            recipient,
            orchard_txid,
            _sapling_txid,
            _transparent_txid,
        ) = faucet_funded_recipient(
            Some(orchard_funds),
            None,
            None,
            Pool::Orchard,
            regtest_network,
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

    pub async fn custom_clients(
        mine_to_pool: Pool,
        regtest_network: zingoconfig::RegtestNetwork,
    ) -> (RegtestManager, ChildProcessHandler, ClientBuilder) {
        let sb = setup::ScenarioBuilder::build_configure_launch(
            Some(mine_to_pool),
            None,
            None,
            &regtest_network,
        )
        .await;
        (
            sb.regtest_manager,
            sb.child_process_handler.unwrap(),
            sb.client_builder,
        )
    }
    pub async fn custom_clients_default() -> (
        RegtestManager,
        ChildProcessHandler,
        ClientBuilder,
        zingoconfig::RegtestNetwork,
    ) {
        let regtest_network = zingoconfig::RegtestNetwork::all_upgrades_active();
        let (regtest_manager, cph, client_builder) =
            custom_clients(Pool::Orchard, regtest_network).await;
        (regtest_manager, cph, client_builder, regtest_network)
    }

    pub async fn unfunded_mobileclient() -> (RegtestManager, ChildProcessHandler) {
        let regtest_network = zingoconfig::RegtestNetwork::all_upgrades_active();
        let scenario_builder = setup::ScenarioBuilder::build_configure_launch(
            None,
            None,
            Some(20_000),
            &regtest_network,
        )
        .await;
        (
            scenario_builder.regtest_manager,
            scenario_builder.child_process_handler.unwrap(),
        )
    }

    pub async fn funded_orchard_mobileclient(value: u64) -> (RegtestManager, ChildProcessHandler) {
        let regtest_network = zingoconfig::RegtestNetwork::all_upgrades_active();
        let mut scenario_builder = setup::ScenarioBuilder::build_configure_launch(
            Some(Pool::Sapling),
            None,
            Some(20_000),
            &regtest_network,
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
        faucet
            .do_send(vec![(
                &get_base_address!(recipient, "unified"),
                value,
                None,
            )])
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

    pub async fn funded_orchard_with_3_txs_mobileclient(
        value: u64,
    ) -> (RegtestManager, ChildProcessHandler) {
        let regtest_network = zingoconfig::RegtestNetwork::all_upgrades_active();
        let mut scenario_builder = setup::ScenarioBuilder::build_configure_launch(
            Some(Pool::Sapling),
            None,
            Some(20_000),
            &regtest_network,
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
        faucet
            .do_send(vec![(
                &get_base_address!(recipient, "unified"),
                value,
                None,
            )])
            .await
            .unwrap();
        increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &recipient, 1)
            .await
            .unwrap();
        // send to a faucet
        recipient
            .do_send(vec![(
                &get_base_address!(faucet, "unified"),
                value.checked_div(10).unwrap(),
                None,
            )])
            .await
            .unwrap();
        increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &recipient, 1)
            .await
            .unwrap();
        // send to self sapling
        recipient
            .do_send(vec![(
                &get_base_address!(recipient, "sapling"),
                value.checked_div(10).unwrap(),
                None,
            )])
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

    pub async fn funded_orchard_sapling_transparent_shielded_mobileclient(
        value: u64,
    ) -> (RegtestManager, ChildProcessHandler) {
        let regtest_network = zingoconfig::RegtestNetwork::all_upgrades_active();
        let mut scenario_builder = setup::ScenarioBuilder::build_configure_launch(
            Some(Pool::Sapling),
            None,
            Some(20_000),
            &regtest_network,
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
        faucet
            .do_send(vec![(
                &get_base_address!(recipient, "unified"),
                value.checked_div(2).unwrap(),
                None,
            )])
            .await
            .unwrap();
        increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &faucet, 1)
            .await
            .unwrap();
        // received from a faucet to sapling
        faucet
            .do_send(vec![(
                &get_base_address!(recipient, "sapling"),
                value.checked_div(4).unwrap(),
                None,
            )])
            .await
            .unwrap();
        increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &faucet, 1)
            .await
            .unwrap();
        // received from a faucet to transparent
        faucet
            .do_send(vec![(
                &get_base_address!(recipient, "transparent"),
                value.checked_div(4).unwrap(),
                None,
            )])
            .await
            .unwrap();
        increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &recipient, 1)
            .await
            .unwrap();
        // send to a faucet
        recipient
            .do_send(vec![(
                &get_base_address!(faucet, "unified"),
                value.checked_div(10).unwrap(),
                None,
            )])
            .await
            .unwrap();
        increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &recipient, 1)
            .await
            .unwrap();
        // send to self orchard
        recipient
            .do_send(vec![(
                &get_base_address!(recipient, "unified"),
                value.checked_div(10).unwrap(),
                None,
            )])
            .await
            .unwrap();
        increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &recipient, 1)
            .await
            .unwrap();
        // send to self sapling
        recipient
            .do_send(vec![(
                &get_base_address!(recipient, "sapling"),
                value.checked_div(10).unwrap(),
                None,
            )])
            .await
            .unwrap();
        increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &recipient, 1)
            .await
            .unwrap();
        // send to self transparent
        recipient
            .do_send(vec![(
                &get_base_address!(recipient, "transparent"),
                value.checked_div(10).unwrap(),
                None,
            )])
            .await
            .unwrap();
        increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &recipient, 1)
            .await
            .unwrap();
        // shield transparent
        recipient
            .do_shield(&[Pool::Transparent], None)
            .await
            .unwrap();
        increase_height_and_wait_for_client(&scenario_builder.regtest_manager, &recipient, 1)
            .await
            .unwrap();
        // upgrade sapling
        recipient.do_shield(&[Pool::Sapling], None).await.unwrap();
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

    pub mod chainload {
        use super::*;

        pub async fn unsynced_basic() -> ChildProcessHandler {
            let regtest_network = zingoconfig::RegtestNetwork::all_upgrades_active();
            setup::ScenarioBuilder::new_load_1153_saplingcb_regtest_chain(&regtest_network)
                .await
                .child_process_handler
                .unwrap()
        }
        pub async fn faucet_recipient_1153() -> (
            RegtestManager,
            ChildProcessHandler,
            LightClient,
            LightClient,
        ) {
            let regtest_network = zingoconfig::RegtestNetwork::all_upgrades_active();
            let mut sb =
                setup::ScenarioBuilder::new_load_1153_saplingcb_regtest_chain(&regtest_network)
                    .await;
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
        pub async fn unsynced_faucet_recipient_1153() -> (
            RegtestManager,
            ChildProcessHandler,
            LightClient,
            LightClient,
        ) {
            let regtest_network = zingoconfig::RegtestNetwork::all_upgrades_active();
            let mut sb =
                setup::ScenarioBuilder::new_load_1153_saplingcb_regtest_chain(&regtest_network)
                    .await;
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
}

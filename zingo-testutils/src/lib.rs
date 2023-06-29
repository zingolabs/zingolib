pub mod data;
pub use incrementalmerkletree;
pub mod regtest;
use std::fs::OpenOptions;
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

fn git_description() -> String {
    std::str::from_utf8(
        &std::process::Command::new("git")
            .arg("describe")
            .arg("--dirty")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .to_string()
    .trim_end_matches('\n')
    .to_string()
}
fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
pub fn timer_annotation(test_name: String) -> JsonValue {
    json::object! { "test_name": test_name, "timestamp": timestamp(), "git_description": git_description() }
}
fn path_to_times(basename: String) -> PathBuf {
    let file_name = PathBuf::from(basename);
    let timing_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("To be inside a manifested space."),
    )
    .join("tests/times");
    timing_dir.join(file_name)
}
pub fn record_time(annotation: &mut JsonValue) {
    let basename = format!("{}.json", annotation.remove("test_name"));
    let data_store = path_to_times(basename);

    let mut data_set = if let Ok(data) = std::fs::read_to_string(data_store.clone()) {
        json::parse(&data).expect("to receive data to be parsed to Json Array")
    } else {
        json::JsonValue::new_array()
    };
    data_set
        .push(annotation.clone())
        .expect("To extend earlier data with new annotation.");

    let mut time_file = OpenOptions::new()
        .create(true)
        .write(true)
        .open(data_store)
        .expect("to access a data_store file");
    std::io::Write::write_all(&mut time_file, data_set.to_string().as_bytes())
        .expect("To write out a new data vector");
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
    dbg!(tips[0]["height"].clone())
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
/// Vheck all the other fields
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
    increase_height_and_sync_client(manager, sender, 1).await?;
    recipient.do_sync(false).await?;
    Ok(txid)
}

// This function increases the chain height reliably (with polling) but
// it _also_ ensures that the client state is synced.
// Unsynced clients are very interesting to us.  See increate_server_height
// to reliably increase the server without syncing the client
pub async fn increase_height_and_sync_client(
    manager: &RegtestManager,
    client: &LightClient,
    n: u32,
) -> Result<(), String> {
    let start_height = get_synced_wallet_height(client).await?;
    let target = start_height + n;
    manager
        .generate_n_blocks(n)
        .expect("Called for side effect, failed!");
    while check_wallet_chainheight_value(client, target).await? {
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
                "{}/zingocli/tests/data/wallets/v26/202302_release/regtest/{nym}/zingo-wallet.dat",
                regtest::get_cargo_manifest_dir_parent().to_string_lossy()
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
    tracing::info!("The wallet is: {}", &wallet.to_str().unwrap());
    let lightwalletd_uri = TestEnvironmentGenerator::new().get_lightwalletd_uri();
    let zingo_config = zingolib::load_clientconfig(lightwalletd_uri, Some(dir), chaintype).unwrap();
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
    //! If you just need a faucet, use the "faucet_only" helper.
    //! If you need a faucet, and a single recipient, use 'faucet_recipient`
    //! For less common client configurations use the client_manager directly with
    //! custom_clients
    use crate::data::{self, seeds::HOSPITAL_MUSEUM_SEED, REGSAP_ADDR_FROM_ABANDONART};

    use super::regtest::{ChildProcessHandler, RegtestManager};
    use zingolib::{get_base_address, lightclient::LightClient};

    use self::setup::ClientManager;

    use super::increase_height_and_sync_client;
    pub mod setup {
        use crate::data::REGSAP_ADDR_FROM_ABANDONART;

        use super::super::regtest::get_regtest_dir;
        use super::{data, ChildProcessHandler, RegtestManager};
        use std::path::PathBuf;
        use zingolib::{lightclient::LightClient, wallet::WalletBase};
        pub struct ScenarioBuilder {
            pub test_env: TestEnvironmentGenerator,
            pub regtest_manager: RegtestManager,
            pub client_builder: ClientManager,
            pub child_process_handler: Option<ChildProcessHandler>,
        }
        impl ScenarioBuilder {
            fn build_scenario(custom_client_config: Option<PathBuf>) -> Self {
                //! TestEnvironmentGenerator sets particular parameters, specific filenames,
                //! port numbers, etc.  in general no test_config should be used for
                //! more than one test, and usually is only invoked via this
                //! ScenarioBuilder::new constructor.  If you need to set some value
                //! once, per test, consider adding environment config (e.g. ports, OS) to
                //! TestEnvironmentGenerator and for scenario specific add to this constructor
                let test_env = TestEnvironmentGenerator::new();
                let regtest_manager = test_env.regtest_manager.clone();
                let data_dir = if let Some(data_dir) = custom_client_config {
                    data_dir
                } else {
                    regtest_manager.zingo_datadir.clone()
                };
                let client_builder = ClientManager::new(
                    test_env.get_lightwalletd_uri(),
                    data_dir,
                    data::seeds::ABANDON_ART_SEED,
                );
                let child_process_handler = None;
                Self {
                    test_env,
                    regtest_manager,
                    client_builder,
                    child_process_handler,
                }
            }
            fn configure_scenario(&mut self, funded: Option<String>) {
                if let Some(funding_seed) = funded {
                    self.test_env.create_funded_zcash_conf(&funding_seed);
                } else {
                    self.test_env.create_unfunded_zcash_conf();
                };
                self.test_env.create_lightwalletd_conf();
            }
            fn launch_scenario(&mut self, clean: bool) {
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
            }
            pub fn new_load_1153_saplingcb_regtest_chain() -> Self {
                let mut sb = ScenarioBuilder::build_scenario(None);
                let source = get_regtest_dir().join("data/chain_cache/blocks_1153/zcashd/regtest");
                let destination = &sb.regtest_manager.zcashd_data_dir;

                std::process::Command::new("cp")
                    .arg("-r")
                    .arg(source)
                    .arg(destination)
                    .output()
                    .expect("copy operation into fresh dir from known dir to succeed");
                dbg!(&sb.test_env.regtest_manager.zcashd_config);
                sb.configure_scenario(Some(REGSAP_ADDR_FROM_ABANDONART.to_string()));
                sb.launch_scenario(false);
                sb
            }

            /// Writes the specified zcashd.conf and launches with it
            pub fn build_configure_launch(
                funded: Option<String>,
                zingo_wallet_dir: Option<PathBuf>,
            ) -> Self {
                let mut sb = if let Some(conf) = zingo_wallet_dir {
                    ScenarioBuilder::build_scenario(Some(conf))
                } else {
                    ScenarioBuilder::build_scenario(None)
                };
                sb.configure_scenario(funded);
                sb.launch_scenario(true);
                sb
            }
        }

        /// Internally (and perhaps in wider scopes) we say "Sprout" to mean
        /// take a seed, and generate a client from the seed (planted in the chain).
        pub struct ClientManager {
            pub server_id: http::Uri,
            pub zingo_datadir: PathBuf,
            seed: String,
            client_number: u8,
        }
        impl ClientManager {
            pub fn new(server_id: http::Uri, zingo_datadir: PathBuf, seed: &str) -> Self {
                let seed = seed.to_string();
                let client_number = 0;
                ClientManager {
                    server_id,
                    zingo_datadir,
                    seed,
                    client_number,
                }
            }
            pub fn make_unique_data_dir_and_load_config(&mut self) -> zingoconfig::ZingoConfig {
                //! Each client requires a unique data_dir, we use the
                //! client_number counter for this.
                self.client_number += 1;
                let conf_path = format!(
                    "{}_client_{}",
                    self.zingo_datadir.to_string_lossy(),
                    self.client_number
                );
                self.create_clientconfig(PathBuf::from(conf_path))
            }
            pub fn create_clientconfig(&self, conf_path: PathBuf) -> zingoconfig::ZingoConfig {
                std::fs::create_dir(&conf_path).unwrap();
                zingolib::load_clientconfig(
                    self.server_id.clone(),
                    Some(conf_path),
                    zingoconfig::ChainType::Regtest,
                )
                .unwrap()
            }

            pub async fn build_new_faucet(
                &mut self,
                birthday: u64,
                overwrite: bool,
            ) -> LightClient {
                //! A "faucet" is a lightclient that receives mining rewards
                let zingo_config = self.make_unique_data_dir_and_load_config();
                LightClient::new_from_wallet_base_async(
                    WalletBase::MnemonicPhrase(self.seed.clone()),
                    &zingo_config,
                    birthday,
                    overwrite,
                )
                .await
                .unwrap()
            }
            pub async fn build_newseed_client(
                &mut self,
                mnemonic_phrase: String,
                birthday: u64,
                overwrite: bool,
            ) -> LightClient {
                let zingo_config = self.make_unique_data_dir_and_load_config();
                LightClient::new_from_wallet_base_async(
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
            pub(crate) fn new() -> Self {
                let zcashd_rpcservice_port = portpicker::pick_unused_port()
                    .expect("Port unpickable!")
                    .to_string();
                let lightwalletd_rpcservice_port = portpicker::pick_unused_port()
                    .expect("Port unpickable!")
                    .to_string();
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
            pub(crate) fn create_unfunded_zcash_conf(&self) -> PathBuf {
                //! Side effect only fn, writes to FS.
                self.write_contents_and_return_path(
                    "zcash",
                    data::config_template_fillers::zcashd::basic(&self.zcashd_rpcservice_port, ""),
                )
            }
            pub(crate) fn create_funded_zcash_conf(&self, address_to_fund: &str) -> PathBuf {
                self.write_contents_and_return_path(
                    "zcash",
                    data::config_template_fillers::zcashd::funded(
                        address_to_fund,
                        &self.zcashd_rpcservice_port,
                    ),
                )
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
    pub fn custom_clients() -> (RegtestManager, ChildProcessHandler, ClientManager) {
        let sb = setup::ScenarioBuilder::build_configure_launch(
            Some(REGSAP_ADDR_FROM_ABANDONART.to_string()),
            None,
        );
        (
            sb.regtest_manager,
            sb.child_process_handler.unwrap(),
            sb.client_builder,
        )
    }
    /// Many scenarios need to start with spendable funds.  This setup provides
    /// 1 block worth of coinbase to a preregistered spend capability.
    ///
    /// This key is registered to receive block rewards by corresponding to the
    /// address registered as the "mineraddress" field in zcash.conf
    ///
    /// The general scenario framework requires instances of zingo-cli, lightwalletd,
    /// and zcashd (in regtest mode). This setup is intended to produce the most basic
    /// of scenarios.  As scenarios with even less requirements
    /// become interesting (e.g. without experimental features, or txindices) we'll create more setups.
    pub async fn faucet() -> (RegtestManager, ChildProcessHandler, LightClient) {
        let mut sb = setup::ScenarioBuilder::build_configure_launch(
            Some(REGSAP_ADDR_FROM_ABANDONART.to_string()),
            None,
        );
        let faucet = sb.client_builder.build_new_faucet(0, false).await;
        faucet.do_sync(false).await.unwrap();
        (
            sb.regtest_manager,
            sb.child_process_handler.unwrap(),
            faucet,
        )
    }

    pub async fn faucet_prefunded_orchard_recipient(
        value: u64,
    ) -> (
        RegtestManager,
        ChildProcessHandler,
        LightClient,
        LightClient,
        String,
    ) {
        let (regtest_manager, child_process_handler, faucet, recipient) = faucet_recipient().await;
        increase_height_and_sync_client(&regtest_manager, &faucet, 1)
            .await
            .unwrap();
        let txid = faucet
            .do_send(vec![(
                &get_base_address!(recipient, "unified"),
                value,
                None,
            )])
            .await
            .unwrap();
        increase_height_and_sync_client(&regtest_manager, &recipient, 1)
            .await
            .unwrap();
        faucet.do_sync(false).await.unwrap();
        (
            regtest_manager,
            child_process_handler,
            faucet,
            recipient,
            txid,
        )
    }

    pub async fn faucet_recipient() -> (
        RegtestManager,
        ChildProcessHandler,
        LightClient,
        LightClient,
    ) {
        let mut sb = setup::ScenarioBuilder::build_configure_launch(
            Some(REGSAP_ADDR_FROM_ABANDONART.to_string()),
            None,
        );
        let faucet = sb.client_builder.build_new_faucet(0, false).await;
        faucet.do_sync(false).await.unwrap();
        let recipient = sb
            .client_builder
            .build_newseed_client(HOSPITAL_MUSEUM_SEED.to_string(), 0, false)
            .await;
        (
            sb.regtest_manager,
            sb.child_process_handler.unwrap(),
            faucet,
            recipient,
        )
    }

    pub async fn basic_no_spendable() -> (RegtestManager, ChildProcessHandler, LightClient) {
        let mut scenario_builder = setup::ScenarioBuilder::build_configure_launch(None, None);
        (
            scenario_builder.regtest_manager,
            scenario_builder.child_process_handler.unwrap(),
            scenario_builder
                .client_builder
                .build_newseed_client(HOSPITAL_MUSEUM_SEED.to_string(), 0, false)
                .await,
        )
    }
    pub mod chainload {
        use super::*;

        pub async fn faucet_recipient_1153() -> (
            RegtestManager,
            ChildProcessHandler,
            LightClient,
            LightClient,
        ) {
            let mut sb = setup::ScenarioBuilder::new_load_1153_saplingcb_regtest_chain();
            //(Some(REGSAP_ADDR_FROM_ABANDONART.to_string()), None);
            let faucet = sb.client_builder.build_new_faucet(0, false).await;
            faucet.do_sync(false).await.unwrap();
            let recipient = sb
                .client_builder
                .build_newseed_client(HOSPITAL_MUSEUM_SEED.to_string(), 0, false)
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
            let mut sb = setup::ScenarioBuilder::new_load_1153_saplingcb_regtest_chain();
            //(Some(REGSAP_ADDR_FROM_ABANDONART.to_string()), None);
            let faucet = sb.client_builder.build_new_faucet(0, false).await;
            let recipient = sb
                .client_builder
                .build_newseed_client(HOSPITAL_MUSEUM_SEED.to_string(), 0, false)
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

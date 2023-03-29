use std::time::Duration;

use json::JsonValue;
use tokio::time::sleep;
use zingo_cli::regtest::RegtestManager;
use zingolib::lightclient::LightClient;

async fn get_synced_wallet_height(client: &LightClient) -> u32 {
    client.do_sync(true).await.unwrap();
    client
        .do_wallet_last_scanned_height()
        .await
        .as_u32()
        .unwrap()
}

fn poll_server_height(manager: &RegtestManager) -> JsonValue {
    let temp_tips = manager.get_chain_tip().unwrap().stdout;
    let tips = json::parse(&String::from_utf8_lossy(&temp_tips)).unwrap();
    dbg!(tips[0]["height"].clone())
}
// This function _DOES NOT SYNC THE CLIENT/WALLET_.
pub async fn increase_server_height(manager: &RegtestManager, n: u32) {
    let start_height = poll_server_height(&manager).as_fixed_point_u64(2).unwrap();
    let target = start_height + n as u64;
    manager
        .generate_n_blocks(n)
        .expect("Called for side effect, failed!");
    let mut count = 0;
    while poll_server_height(&manager).as_fixed_point_u64(2).unwrap() < target {
        sleep(Duration::from_millis(50)).await;
        count = dbg!(count + 1);
    }
}

pub async fn send_value_between_clients_and_sync(
    manager: &RegtestManager,
    sender: &LightClient,
    recipient: &LightClient,
    value: u64,
    pool: &str,
) -> String {
    let txid = sender
        .do_send(vec![(
            &zingolib::get_base_address!(recipient, pool),
            value,
            None,
        )])
        .await
        .unwrap();
    increase_height_and_sync_client(manager, sender, 1).await;
    recipient.do_sync(false).await.unwrap();
    txid
}

// This function increases the chain height reliably (with polling) but
// it _also_ ensures that the client state is synced.
// Unsynced clients are very interesting to us.  See increate_server_height
// to reliably increase the server without syncing the client
pub async fn increase_height_and_sync_client(
    manager: &RegtestManager,
    client: &LightClient,
    n: u32,
) {
    let start_height = get_synced_wallet_height(&client).await;
    let target = start_height + n;
    manager
        .generate_n_blocks(n)
        .expect("Called for side effect, failed!");
    while check_wallet_chainheight_value(&client, target).await {
        sleep(Duration::from_millis(50)).await;
    }
}
async fn check_wallet_chainheight_value(client: &LightClient, target: u32) -> bool {
    get_synced_wallet_height(&client).await != target
}
#[cfg(test)]
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

    use zingo_cli::regtest::{ChildProcessHandler, RegtestManager};
    use zingolib::{get_base_address, lightclient::LightClient};

    use self::setup::ClientManager;

    use super::increase_height_and_sync_client;
    pub mod setup {
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
            fn new(custom_client_config: Option<String>) -> Self {
                //! TestEnvironmentGenerator sets particular parameters, specific filenames,
                //! port numbers, etc.  in general no test_config should be used for
                //! more than one test, and usually is only invoked via this
                //! ScenarioBuilder::new constructor.  If you need to set some value
                //! once, per test, consider adding environment config (e.g. ports, OS) to
                //! TestEnvironmentGenerator and for scenario specific add to this constructor
                let test_env = TestEnvironmentGenerator::new();
                let regtest_manager = test_env.regtest_manager.clone();
                let lightwalletd_port = test_env.lightwalletd_rpcservice_port.clone();
                let server_id = zingoconfig::construct_server_uri(Some(format!(
                    "http://127.0.0.1:{lightwalletd_port}"
                )));
                let data_dir = if let Some(data_dir) = custom_client_config {
                    data_dir
                } else {
                    regtest_manager
                        .zingo_datadir
                        .clone()
                        .to_string_lossy()
                        .to_string()
                };
                let client_builder =
                    ClientManager::new(server_id, data_dir, data::seeds::ABANDON_ART_SEED);
                let child_process_handler = None;
                Self {
                    test_env,
                    regtest_manager,
                    client_builder,
                    child_process_handler,
                }
            }
            fn launch(&mut self) {
                self.child_process_handler = Some(
                    self.regtest_manager
                        .launch(true)
                        .unwrap_or_else(|e| match e {
                            zingo_cli::regtest::LaunchChildProcessError::ZcashdState {
                                errorcode,
                                stdout,
                                stderr,
                            } => {
                                panic!("{} {} {}", errorcode, stdout, stderr)
                            }
                        }),
                );
            }
            pub fn launcher(funded: Option<String>, custom_conf: Option<String>) -> Self {
                let mut sb = if let Some(conf) = custom_conf {
                    ScenarioBuilder::new(Some(conf))
                } else {
                    ScenarioBuilder::new(None)
                };
                if let Some(funding_seed) = funded {
                    sb.test_env.create_funded_zcash_conf(&funding_seed);
                } else {
                    sb.test_env.create_unfunded_zcash_conf();
                };
                sb.test_env.create_lightwalletd_conf();
                sb.launch();
                sb
            }
        }

        /// Internally (and perhaps in wider scopes) we say "Sprout" to mean
        /// take a seed, and generate a client from the seed (planted in the chain).
        pub struct ClientManager {
            pub server_id: http::Uri,
            pub zingo_datadir: String,
            seed: String,
            client_number: u8,
        }
        impl ClientManager {
            pub fn new(server_id: http::Uri, zingo_datadir: String, seed: &str) -> Self {
                let seed = seed.to_string();
                let client_number = 0;
                ClientManager {
                    server_id,
                    zingo_datadir,
                    seed,
                    client_number,
                }
            }
            pub async fn make_unique_data_dir_and_load_config(
                &mut self,
            ) -> (zingoconfig::ZingoConfig, u64) {
                //! Each client requires a unique data_dir, we use the
                //! client_number counter for this.
                self.client_number += 1;
                let conf_path = format!("{}_client_{}", self.zingo_datadir, self.client_number);
                self.create_clientconfig(conf_path).await
            }
            pub async fn create_clientconfig(
                &self,
                conf_path: String,
            ) -> (zingoconfig::ZingoConfig, u64) {
                std::fs::create_dir(&conf_path).unwrap();
                zingolib::load_clientconfig_async(self.server_id.clone(), Some(conf_path))
                    .await
                    .unwrap()
            }

            pub async fn build_new_faucet(
                &mut self,
                birthday: u64,
                overwrite: bool,
            ) -> LightClient {
                //! A "faucet" is a lightclient that receives mining rewards
                let (zingo_config, _) = self.make_unique_data_dir_and_load_config().await;
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
                let (zingo_config, _) = self.make_unique_data_dir_and_load_config().await;
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
        }
        impl TestEnvironmentGenerator {
            fn new() -> Self {
                let mut common_path = zingo_cli::regtest::get_git_rootdir();
                common_path.push("cli");
                common_path.push("tests");
                common_path.push("data");
                let zcashd_rpcservice_port = portpicker::pick_unused_port()
                    .expect("Port unpickable!")
                    .to_string();
                let lightwalletd_rpcservice_port = portpicker::pick_unused_port()
                    .expect("Port unpickable!")
                    .to_string();
                let regtest_manager = RegtestManager::new(Some(
                    tempdir::TempDir::new("zingo_integration_test")
                        .unwrap()
                        .into_path(),
                ));
                Self {
                    zcashd_rpcservice_port,
                    lightwalletd_rpcservice_port,
                    regtest_manager,
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
                let mut output = std::fs::File::create(&loc).expect("How could path be missing?");
                std::io::Write::write(&mut output, contents.as_bytes())
                    .expect(&format!("Couldn't write {contents}!"));
                loc.clone()
            }
        }
    }
    pub fn custom_clients() -> (RegtestManager, ChildProcessHandler, ClientManager) {
        let sb =
            setup::ScenarioBuilder::launcher(Some(REGSAP_ADDR_FROM_ABANDONART.to_string()), None);
        (
            sb.regtest_manager,
            sb.child_process_handler.unwrap(),
            sb.client_builder,
        )
    }
    pub fn custom_config(config: &str) -> (RegtestManager, ChildProcessHandler, ClientManager) {
        let sb = setup::ScenarioBuilder::launcher(
            Some(REGSAP_ADDR_FROM_ABANDONART.to_string()),
            Some(config.to_string()),
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
        let mut sb =
            setup::ScenarioBuilder::launcher(Some(REGSAP_ADDR_FROM_ABANDONART.to_string()), None);
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
        increase_height_and_sync_client(&regtest_manager, &faucet, 1).await;
        let txid = faucet
            .do_send(vec![(
                &get_base_address!(recipient, "unified"),
                value.into(),
                None,
            )])
            .await
            .unwrap();
        increase_height_and_sync_client(&regtest_manager, &recipient, 1).await;
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
        let mut sb =
            setup::ScenarioBuilder::launcher(Some(REGSAP_ADDR_FROM_ABANDONART.to_string()), None);
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
    #[cfg(feature = "cross_version")]
    pub async fn current_and_fixed_clients() -> (
        RegtestManager,
        ChildProcessHandler,
        LightClient,
        zingtaddrfix::lightclient::LightClient,
    ) {
        //tracing_subscriber::fmt::init();
        let cross_version_seed_phrase = zcash_primitives::zip339::Mnemonic::from_entropy([3; 32])
            .unwrap()
            .to_string();
        assert_eq!(
            &cross_version_seed_phrase,
            "adapt blossom school alcohol coral light army gather \
             adapt blossom school alcohol coral light army gather \
             adapt blossom school alcohol coral light army hold"
        );
        let first_z_addr_from_seed_phrase = "zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p";
        let mut scenario_builder =
            setup::ScenarioBuilder::launcher(Some(first_z_addr_from_seed_phrase.to_string()), None);
        let current_client = scenario_builder
            .client_builder
            .build_newseed_client(cross_version_seed_phrase.clone(), 0, false)
            .await;
        // Fixed client creation
        let conf_path = scenario_builder
            .regtest_manager
            .zingo_datadir
            .clone()
            .into_os_string()
            .into_string()
            .unwrap();
        let (fixed_zingoconfig, _) = zingtaddrfix::create_zingoconfdir_async(
            scenario_builder.client_builder.server_id.clone(),
            Some(conf_path),
        )
        .await
        .unwrap();
        let fixed_client = zingtaddrfix::lightclient::LightClient::new_from_wallet_base_async(
            zingtaddrfix::wallet::WalletBase::MnemonicPhrase(cross_version_seed_phrase.clone()),
            &fixed_zingoconfig,
            0,
            false,
        )
        .await
        .unwrap();
        (
            scenario_builder.regtest_manager,
            scenario_builder.child_process_handler.unwrap(),
            current_client,
            fixed_client,
        )
    }

    pub async fn basic_no_spendable() -> (RegtestManager, ChildProcessHandler, LightClient) {
        let mut scenario_builder = setup::ScenarioBuilder::launcher(None, None);
        (
            scenario_builder.regtest_manager,
            scenario_builder.child_process_handler.unwrap(),
            scenario_builder
                .client_builder
                .build_newseed_client(HOSPITAL_MUSEUM_SEED.to_string(), 0, false)
                .await,
        )
    }
}

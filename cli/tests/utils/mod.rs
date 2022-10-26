use std::time::Duration;

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
pub mod setup {
    use crate::data;
    use std::path::PathBuf;

    use zingo_cli::regtest::{ChildProcessHandler, RegtestManager};
    use zingolib::{create_zingoconf_with_datadir, lightclient::LightClient};

    ///  Test setup involves common configurations files.  Contents and locations
    ///  are variable.
    ///   Locations:
    ///     Each test must have a unique set of config files.  By default those
    ///     files will be preserved on test failure.
    ///   Contents:
    ///     The specific configuration values may or may not differ between
    ///     scenarios and/or tests.
    ///     Data templates for config files are in:
    ///        * tests::data::config_template_fillers::zcashd
    ///        * tests::data::config_template_fillers::lightwalletd
    struct TestConfigGenerator {
        zcashd_rpcservice_port: String,
        lightwalletd_rpcservice_port: String,
        regtest_manager: RegtestManager,
    }
    impl TestConfigGenerator {
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

        fn create_unfunded_zcash_conf(&self) -> PathBuf {
            self.write_contents_and_return_path(
                "zcash",
                data::config_template_fillers::zcashd::basic(&self.zcashd_rpcservice_port, ""),
            )
        }
        fn create_funded_zcash_conf(&self, address_to_fund: &str) -> PathBuf {
            self.write_contents_and_return_path(
                "zcash",
                data::config_template_fillers::zcashd::funded(
                    address_to_fund,
                    &self.zcashd_rpcservice_port,
                ),
            )
        }
        fn create_lightwalletd_conf(&self) -> PathBuf {
            self.write_contents_and_return_path(
                "lightwalletd",
                dbg!(data::config_template_fillers::lightwalletd::basic(
                    &self.lightwalletd_rpcservice_port
                )),
            )
        }
        fn write_contents_and_return_path(&self, configtype: &str, contents: String) -> PathBuf {
            let loc = match configtype {
                "zcash" => &self.regtest_manager.zcashd_config,
                "lightwalletd" => &self.regtest_manager.lightwalletd_config,
                _ => panic!("Unepexted configtype!"),
            };
            dbg!(&loc);
            let mut output = std::fs::File::create(&loc).expect("How could path be missing?");
            std::io::Write::write(&mut output, contents.as_bytes())
                .expect(&format!("Couldn't write {contents}!"));
            loc.clone()
        }
    }
    fn create_maybe_funded_regtest_manager(
        fund_recipient_address: Option<&str>,
    ) -> (RegtestManager, String) {
        let test_configs = TestConfigGenerator::new();
        match fund_recipient_address {
            Some(fund_to_address) => test_configs.create_funded_zcash_conf(fund_to_address),
            None => test_configs.create_unfunded_zcash_conf(),
        };
        test_configs.create_lightwalletd_conf();
        (
            test_configs.regtest_manager,
            test_configs.lightwalletd_rpcservice_port,
        )
    }

    /// Many scenarios need to start with spendable funds.  This setup provides
    /// 1 block worth of coinbase to a preregistered spend capability.
    ///
    /// This key is registered to receive block rewards by corresponding to the
    /// address registered as the "mineraddress" field in cli/examples/zcash.conf
    ///
    /// The general scenario framework requires instances of zingo-cli, lightwalletd,  
    /// and zcashd (in regtest mode). This setup is intended to produce the most basic  
    /// of scenarios.  As scenarios with even less requirements
    /// become interesting (e.g. without experimental features, or txindices) we'll create more setups.
    pub fn saplingcoinbasebacked_spendcapable() -> (RegtestManager, ChildProcessHandler, LightClient)
    {
        //tracing_subscriber::fmt::init();
        let seed_phrase = zcash_primitives::zip339::Mnemonic::from_entropy([0; 32])
            .unwrap()
            .to_string();
        assert_eq!(
            &seed_phrase,
            "abandon abandon abandon abandon abandon abandon abandon abandon \
             abandon abandon abandon abandon abandon abandon abandon abandon \
             abandon abandon abandon abandon abandon abandon abandon art"
        );
        let first_z_addr_from_seed_phrase = "zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p";
        let (regtest_manager, server_port) =
            create_maybe_funded_regtest_manager(Some(first_z_addr_from_seed_phrase));
        let child_process_handler = regtest_manager.launch(true).unwrap_or_else(|e| match e {
            zingo_cli::regtest::LaunchChildProcessError::ZcashdState {
                errorcode,
                stdout,
                stderr,
            } => {
                panic!("{} {} {}", errorcode, stdout, stderr)
            }
        });
        let server_id =
            zingoconfig::construct_server_uri(Some(format!("http://127.0.0.1:{server_port}")));
        let (config, _height) = create_zingoconf_with_datadir(
            server_id,
            Some(regtest_manager.zingo_data_dir.to_string_lossy().to_string()),
        )
        .unwrap();
        let light_client =
            LightClient::create_with_seedorkey_wallet(seed_phrase, &config, 0, false).unwrap();
        (regtest_manager, child_process_handler, light_client)
    }
    /// This creates two so-called "LightClient"s "client_one" controls a spend capability
    /// that has furnished a receiving address in the mineraddress configuration field
    /// of the "generating" regtest-zcashd
    pub fn two_clients_one_saplingcoinbase_backed() -> (
        RegtestManager,
        LightClient,
        LightClient,
        ChildProcessHandler,
    ) {
        let (regtest_manager, child_process_handler, client_one) =
            saplingcoinbasebacked_spendcapable();
        let client_two_zingoconf_path = format!(
            "{}_two",
            regtest_manager.zingo_data_dir.to_string_lossy().to_string()
        );
        std::fs::create_dir(&client_two_zingoconf_path).unwrap();
        let (client_two_config, _height) = create_zingoconf_with_datadir(
            client_one.get_server_uri(),
            Some(client_two_zingoconf_path),
        )
        .unwrap();
        let seed_phrase_of_two = zcash_primitives::zip339::Mnemonic::from_entropy([1; 32])
            .unwrap()
            .to_string();
        let client_two = LightClient::create_with_seedorkey_wallet(
            seed_phrase_of_two,
            &client_two_config,
            0,
            false,
        )
        .unwrap();
        (
            regtest_manager,
            client_one,
            client_two,
            child_process_handler,
        )
    }

    pub fn basic_no_spendable() -> (RegtestManager, ChildProcessHandler, LightClient) {
        let (regtest_manager, server_port) = create_maybe_funded_regtest_manager(None);
        let child_process_handler = regtest_manager.launch(true).unwrap();
        let server_id =
            zingoconfig::construct_server_uri(Some(format!("http://127.0.0.1:{server_port}")));
        let (config, _height) = create_zingoconf_with_datadir(
            server_id,
            Some(regtest_manager.zingo_data_dir.to_string_lossy().to_string()),
        )
        .unwrap();
        (
            regtest_manager,
            child_process_handler,
            LightClient::new(&config, 0).unwrap(),
        )
    }
}

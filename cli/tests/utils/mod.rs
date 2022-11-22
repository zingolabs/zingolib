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
pub mod setup {
    use crate::data;
    use std::path::PathBuf;

    use zingo_cli::regtest::{ChildProcessHandler, RegtestManager};
    use zingolib::{create_zingoconf_with_datadir, lightclient::LightClient};

    pub struct ClientBuilder {
        server_id: http::Uri,
        zingo_datadir: PathBuf,
        seed: Option<String>,
        client_number: u8,
    }
    impl ClientBuilder {
        pub fn new(server_id: http::Uri, zingo_datadir: PathBuf, seed: Option<String>) -> Self {
            let client_number = 0;
            ClientBuilder {
                server_id,
                zingo_datadir,
                seed,
                client_number,
            }
        }
        fn make_config(&mut self) -> (zingoconfig::ZingoConfig, u64) {
            self.client_number += 1;
            let conf_path = format!(
                "{}_{}",
                self.zingo_datadir.to_string_lossy().to_string(),
                self.client_number
            );
            std::fs::create_dir(&conf_path).unwrap();

            zingolib::create_zingoconf_with_datadir(self.server_id.clone(), Some(conf_path))
                .unwrap()
        }
        pub fn new_sameseed_client(&mut self, birthday: u64, overwrite: bool) -> LightClient {
            let (zingo_config, _) = self.make_config();
            LightClient::create_with_seedorkey_wallet(
                self.seed.clone().unwrap(),
                &zingo_config,
                birthday,
                overwrite,
            )
            .unwrap()
        }

        pub fn new_plantedseed_client(
            &mut self,
            seed: String,
            birthday: u64,
            overwrite: bool,
        ) -> LightClient {
            let (zingo_config, _) = self.make_config();
            LightClient::create_with_seedorkey_wallet(seed, &zingo_config, birthday, overwrite)
                .unwrap()
        }
    }
    //pub fn add_nonprimary_client() -> LightClient {}
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
                data::config_template_fillers::lightwalletd::basic(
                    &self.lightwalletd_rpcservice_port,
                ),
            )
        }
        fn write_contents_and_return_path(&self, configtype: &str, contents: String) -> PathBuf {
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
    /// address registered as the "mineraddress" field in zcash.conf
    ///
    /// The general scenario framework requires instances of zingo-cli, lightwalletd,  
    /// and zcashd (in regtest mode). This setup is intended to produce the most basic  
    /// of scenarios.  As scenarios with even less requirements
    /// become interesting (e.g. without experimental features, or txindices) we'll create more setups.
    pub fn saplingcoinbasebacked_spendcapable(
    ) -> (RegtestManager, ChildProcessHandler, ClientBuilder) {
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
        let (regtest_manager, lightwalletd_port) =
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
        let server_id = zingoconfig::construct_server_uri(Some(format!(
            "http://127.0.0.1:{lightwalletd_port}"
        )));
        let client_builder = ClientBuilder::new(
            server_id,
            regtest_manager.zingo_data_dir.clone(),
            Some(seed_phrase),
        );
        (regtest_manager, child_process_handler, client_builder)
    }

    #[cfg(feature = "cross_version")]
    pub fn saplingcoinbasebacked_spendcapable_cross_version(
    ) -> (RegtestManager, ChildProcessHandler, LightClient, String) {
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
        let (regtest_manager, lightwalletd_port) =
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
        let server_id = zingoconfig::construct_server_uri(Some(format!(
            "http://127.0.0.1:{lightwalletd_port}"
        )));
        let (config, _height) = create_zingoconf_with_datadir(
            server_id,
            Some(regtest_manager.zingo_data_dir.to_string_lossy().to_string()),
        )
        .unwrap();
        let light_client = LightClient::create_with_seedorkey_wallet(
            cross_version_seed_phrase.clone(),
            &config,
            0,
            false,
        )
        .unwrap();
        (
            regtest_manager,
            child_process_handler,
            light_client,
            cross_version_seed_phrase,
        )
    }
    /// This creates two so-called "LightClient"s "client_one" controls a spend capability
    /// that has furnished a receiving address in the mineraddress configuration field
    /// of the "generating" regtest-zcashd
    pub fn two_clients_one_saplingcoinbase_backed() -> (
        RegtestManager,
        LightClient,
        LightClient,
        ChildProcessHandler,
        ClientBuilder,
    ) {
        let (regtest_manager, child_process_handler, mut client_builder) =
            saplingcoinbasebacked_spendcapable();
        let client_one = client_builder.new_sameseed_client(0, false);
        let seed_phrase_of_two = zcash_primitives::zip339::Mnemonic::from_entropy([1; 32])
            .unwrap()
            .to_string();
        let client_two = client_builder.new_plantedseed_client(seed_phrase_of_two, 0, false);
        (
            regtest_manager,
            client_one,
            client_two,
            child_process_handler,
            client_builder,
        )
    }

    /// This creates two so-called "LightClient"s "client_one" controls a spend capability
    /// that has furnished a receiving address in the mineraddress configuration field
    /// of the "generating" regtest-zcashd
    #[cfg(feature = "cross_version")]
    pub fn cross_version_setup() -> (
        RegtestManager,
        LightClient,
        zingtaddrfix::lightclient::LightClient,
        ChildProcessHandler,
    ) {
        let (regtest_manager, child_process_handler, current_client, current_seed_phrase) =
            saplingcoinbasebacked_spendcapable_cross_version();
        let current_version_client_zingoconf_path = format!(
            "{}_two",
            regtest_manager.zingo_data_dir.to_string_lossy().to_string()
        );
        std::fs::create_dir(&current_version_client_zingoconf_path).unwrap();
        let (indexed_taddr_client, _height) = zingtaddrfix::create_zingoconf_with_datadir(
            current_client.get_server_uri(),
            Some(current_version_client_zingoconf_path),
        )
        .unwrap();
        let fixed_taddr_client =
            zingtaddrfix::lightclient::LightClient::create_with_seedorkey_wallet(
                current_seed_phrase,
                &indexed_taddr_client,
                0,
                false,
            )
            .unwrap();
        (
            regtest_manager,
            current_client,
            fixed_taddr_client,
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

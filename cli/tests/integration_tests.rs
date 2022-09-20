#![forbid(unsafe_code)]
use std::time::Duration;

mod data;
use tokio::{runtime::Runtime, time::sleep};
use zingo_cli::regtest::{ChildProcessHandler, RegtestManager};
use zingoconfig::ZingoConfig;
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
    zcash_conf_location: std::path::PathBuf,
    lightwalletd_conf_location: std::path::PathBuf,
    zcashd_chain_port: u16,
}
impl TestConfigGenerator {
    fn new(zcash_pathbase: &str, lightwalletd_pathbase: &str) -> Self {
        let mut common_path = zingo_cli::regtest::get_git_rootdir();
        common_path.push("cli");
        common_path.push("tests");
        common_path.push("data");
        let zcash_conf_location = common_path.join(zcash_pathbase);
        let lightwalletd_conf_location = common_path.join(lightwalletd_pathbase);
        let zcashd_chain_port = portpicker::pick_unused_port().expect("Port unpickable!");
        Self {
            zcash_conf_location,
            lightwalletd_conf_location,
            zcashd_chain_port,
        }
    }
    fn create_funded_zcash_conf(&self, address_to_fund: &str) -> std::path::PathBuf {
        self.write_contents_and_return_path(
            "zcash",
            data::config_template_fillers::zcashd::funded(
                address_to_fund,
                dbg!(format!("{:?}", self.zcashd_chain_port).as_str()),
            ),
        )
    }
    fn create_lightwalletd_conf(&self) -> std::path::PathBuf {
        self.write_contents_and_return_path(
            "lightwalletd",
            data::config_template_fillers::lightwalletd::basic(),
        )
    }
    fn write_contents_and_return_path(
        &self,
        configtype: &str,
        contents: String,
    ) -> std::path::PathBuf {
        let loc = match configtype {
            "zcash" => &self.zcash_conf_location,
            "lightwalletd" => &self.lightwalletd_conf_location,
            _ => panic!("Unepexted configtype!"),
        };
        let mut output = std::fs::File::create(&loc).expect("How could path {config} be missing?");
        std::io::Write::write(&mut output, contents.as_bytes())
            .expect("Couldn't write {contents}!");
        loc.clone()
    }
}
fn create_funded_regtest_manager(
    zcash_pathbase: &str,
    lightwalletd_pathbase: &str,
    fund_recipient_address: &str,
) -> RegtestManager {
    let test_configs = TestConfigGenerator::new(zcash_pathbase, lightwalletd_pathbase);
    RegtestManager::new(
        Some(test_configs.create_funded_zcash_conf(fund_recipient_address)),
        Some(test_configs.create_lightwalletd_conf()),
    )
}
/// The general scenario framework requires instances of zingo-cli, lightwalletd, and zcashd (in regtest mode).
/// This setup is intended to produce the most basic of scenarios.  As scenarios with even less requirements
/// become interesting (e.g. without experimental features, or txindices) we'll create more setups.
fn basic_zcashd_lwd_zingolib_connected_setup() -> (RegtestManager, ChildProcessHandler, LightClient)
{
    let regtest_manager = create_funded_regtest_manager(
        "basic_zcashd.conf",
        "lightwalletd.yml",
        data::SAPLING_ADDRESS_FROM_SPEND_AUTH,
    );
    let child_process_handler = regtest_manager.launch(true).unwrap();
    let server_id = ZingoConfig::get_server_or_default(Some("http://127.0.0.1".to_string()));
    let (config, _height) = create_zingoconf_with_datadir(
        server_id,
        Some(regtest_manager.zingo_datadir.to_string_lossy().to_string()),
    )
    .unwrap();
    (
        regtest_manager,
        child_process_handler,
        LightClient::new(&config, 0).unwrap(),
    )
}
#[ignore]
#[test]
fn basic_connectivity_scenario() {
    let _regtest_manager = basic_zcashd_lwd_zingolib_connected_setup();
}
/// Many scenarios need to start with spendable funds.  This setup provides
/// 1 block worth of coinbase to a preregistered spend capability.
///
/// This key is registered to receive block rewards by:
///  (1) existing accessibly for test code in: cli/examples/mineraddress_sapling_spendingkey
///  (2) corresponding to the address registered as the "mineraddress" field in cli/examples/zcash.conf
fn coinbasebacked_spendcapable_setup() -> (RegtestManager, ChildProcessHandler, LightClient) {
    //tracing_subscriber::fmt::init();
    let coinbase_spendkey = include_str!("data/mineraddress_sapling_spendingkey").to_string();
    let regtest_manager = create_funded_regtest_manager(
        "externalwallet_coinbaseaddress.conf",
        "lightwalletd.yml",
        data::SAPLING_ADDRESS_FROM_SPEND_AUTH,
    );
    let child_process_handler = regtest_manager.launch(true).unwrap();
    let server_id = ZingoConfig::get_server_or_default(Some("http://127.0.0.1".to_string()));
    let (config, _height) = create_zingoconf_with_datadir(
        server_id,
        Some(regtest_manager.zingo_datadir.to_string_lossy().to_string()),
    )
    .unwrap();
    (
        regtest_manager,
        child_process_handler,
        LightClient::create_with_capable_wallet(coinbase_spendkey, &config, 0, false).unwrap(),
    )
}

#[test]
fn empty_zcashd_sapling_commitment_tree() {
    let (regtest_manager, _child_process_handler, client) = coinbasebacked_spendcapable_setup();
    let trees = regtest_manager
        .get_cli_handle()
        .args(["z_gettreestate", "1"])
        .output()
        .expect("Couldn't get the trees.");
    let trees = json::parse(&String::from_utf8_lossy(&trees.stdout));
    let pretty_trees = json::stringify_pretty(trees.unwrap(), 4);
    println!("{}", pretty_trees);
}

#[ignore]
#[test]
fn mine_sapling_to_self_a() {
    let (regtest_manager, _child_process_handler, client) = coinbasebacked_spendcapable_setup();
    regtest_manager.generate_n_blocks(5).unwrap();
    let runtime = Runtime::new().unwrap();

    runtime.block_on(client.do_sync(true)).unwrap();

    let balance = runtime.block_on(client.do_balance());
    assert_eq!(balance["sapling_balance"], 625000000);
}

#[ignore]
#[test]
fn send_mined_sapling_to_orchard() {
    let (regtest_manager, _child_process_handler, client) = coinbasebacked_spendcapable_setup();
    regtest_manager.generate_n_blocks(5).unwrap();
    let runtime = Runtime::new().unwrap();
    runtime.block_on(async {
        sleep(Duration::from_secs(2)).await;
        let sync_status = client.do_sync(true).await.unwrap();
        println!("{}", json::stringify_pretty(sync_status, 4));

        let o_addr = client.do_new_address("o").await.unwrap()[0].take();
        println!("{o_addr}");
        let send_status = client
            .do_send(vec![(
                o_addr.to_string().as_str(),
                5000,
                Some("Scenario test: engage!".to_string()),
            )])
            .await
            .unwrap();
        println!("Send status: {send_status}");

        regtest_manager.generate_n_blocks(2).unwrap();
        sleep(Duration::from_secs(2)).await;

        client.do_sync(true).await.unwrap();
        let balance = client.do_balance().await;
        let transactions = client.do_list_transactions(false).await;
        println!("{}", json::stringify_pretty(balance.clone(), 4));
        println!("{}", json::stringify_pretty(transactions, 4));
        assert_eq!(balance["unverified_orchard_balance"], 5000);
        assert_eq!(balance["verified_orchard_balance"], 0);

        regtest_manager.generate_n_blocks(4).unwrap();
        sleep(Duration::from_secs(2)).await;
        client.do_sync(true).await.unwrap();
        let balance = client.do_balance().await;
        println!("{}", json::stringify_pretty(balance.clone(), 4));
        assert_eq!(balance["unverified_orchard_balance"], 0);
        assert_eq!(balance["verified_orchard_balance"], 5000);
    });
}

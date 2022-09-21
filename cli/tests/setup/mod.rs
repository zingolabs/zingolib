use crate::data;
use std::path::PathBuf;

use tokio::runtime::Runtime;
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
    zcash_conf_location: PathBuf,
    lightwalletd_conf_location: PathBuf,
    zcashd_rpcservice_port: String,
    lightwalletd_rpcservice_port: String,
}
impl TestConfigGenerator {
    fn new(zcash_pathbase: &str, lightwalletd_pathbase: &str) -> Self {
        let mut common_path = zingo_cli::regtest::get_git_rootdir();
        common_path.push("cli");
        common_path.push("tests");
        common_path.push("data");
        let zcash_conf_location = common_path.join(zcash_pathbase);
        let lightwalletd_conf_location = common_path.join(lightwalletd_pathbase);
        let zcashd_rpcservice_port = portpicker::pick_unused_port()
            .expect("Port unpickable!")
            .to_string();
        let lightwalletd_rpcservice_port = portpicker::pick_unused_port()
            .expect("Port unpickable!")
            .to_string();
        Self {
            zcash_conf_location,
            lightwalletd_conf_location,
            zcashd_rpcservice_port,
            lightwalletd_rpcservice_port,
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
fn create_maybe_funded_regtest_manager(
    zcash_pathbase: &str,
    lightwalletd_pathbase: &str,
    fund_recipient_address: Option<&str>,
) -> RegtestManager {
    let test_configs = TestConfigGenerator::new(zcash_pathbase, lightwalletd_pathbase);
    RegtestManager::new(
        Some(match fund_recipient_address {
            Some(fund_to_address) => test_configs.create_funded_zcash_conf(fund_to_address),
            None => test_configs.create_unfunded_zcash_conf(),
        }),
        Some(test_configs.create_lightwalletd_conf()),
    )
}
/// The general scenario framework requires instances of zingo-cli, lightwalletd, and zcashd (in regtest mode).
/// This setup is intended to produce the most basic of scenarios.  As scenarios with even less requirements
/// become interesting (e.g. without experimental features, or txindices) we'll create more setups.
pub fn basic_funded_zcashd_lwd_zingolib_connected(
) -> (RegtestManager, ChildProcessHandler, LightClient) {
    let regtest_manager = create_maybe_funded_regtest_manager(
        "basic_zcashd.conf",
        "lightwalletd.yml",
        Some(data::SAPLING_ADDRESS_FROM_SPEND_AUTH),
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
/// Many scenarios need to start with spendable funds.  This setup provides
/// 1 block worth of coinbase to a preregistered spend capability.
///
/// This key is registered to receive block rewards by:
///  (1) existing accessibly for test code in: cli/examples/mineraddress_sapling_spendingkey
///  (2) corresponding to the address registered as the "mineraddress" field in cli/examples/zcash.conf
pub fn coinbasebacked_spendcapable() -> (RegtestManager, ChildProcessHandler, LightClient, Runtime)
{
    //tracing_subscriber::fmt::init();
    let coinbase_spendkey = data::SECRET_SPEND_AUTH_SAPLING.to_string();
    let regtest_manager = create_maybe_funded_regtest_manager(
        "externalwallet_coinbaseaddress.conf",
        "lightwalletd.yml",
        Some(data::SAPLING_ADDRESS_FROM_SPEND_AUTH),
    );
    let child_process_handler = regtest_manager.launch(true).unwrap();
    let server_id = ZingoConfig::get_server_or_default(Some("http://127.0.0.1".to_string()));
    let (config, _height) = create_zingoconf_with_datadir(
        server_id,
        Some(regtest_manager.zingo_datadir.to_string_lossy().to_string()),
    )
    .unwrap();
    regtest_manager.generate_n_blocks(5).unwrap();
    (
        regtest_manager,
        child_process_handler,
        LightClient::create_with_capable_wallet(coinbase_spendkey, &config, 0, false).unwrap(),
        Runtime::new().unwrap(),
    )
}

pub fn basic_no_spendable() -> (RegtestManager, ChildProcessHandler, LightClient) {
    let regtest_manager = create_maybe_funded_regtest_manager(
        "externalwallet_coinbaseaddress.conf",
        "lightwalletd.yml",
        None,
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

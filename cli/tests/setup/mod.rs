use crate::data;
use rand::{rngs::OsRng, Rng};
use std::path::PathBuf;

use tokio::runtime::Runtime;
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
/// This key is registered to receive block rewards by:
///  (1) existing accessibly for test code in: cli/examples/mineraddress_sapling_spendingkey
///  (2) corresponding to the address registered as the "mineraddress" field in cli/examples/zcash.conf
///
/// The general scenario framework requires instances of zingo-cli, lightwalletd, and zcashd (in regtest mode).
/// This setup is intended to produce the most basic of scenarios.  As scenarios with even less requirements
/// become interesting (e.g. without experimental features, or txindices) we'll create more setups.
pub fn coinbasebacked_spendcapable() -> (RegtestManager, ChildProcessHandler, LightClient, Runtime)
{
    //tracing_subscriber::fmt::init();
    let coinbase_spendkey =
        zcash_primitives::zip32::ExtendedSpendingKey::master(&OsRng.gen::<[u8; 32]>());
    let (regtest_manager, server_port) = create_maybe_funded_regtest_manager(Some(
        &zcash_client_backend::encoding::encode_payment_address(
            "zregtestsapling",
            &coinbase_spendkey.default_address().1,
        ),
    ));
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
    let mut spendkey_bytes = Vec::new();
    coinbase_spendkey.write(&mut spendkey_bytes).unwrap();
    let light_client = LightClient::create_with_capable_wallet(
        bech32::encode(
            "secret-extended-key-regtest",
            <Vec<u8> as bech32::ToBase32>::to_base32(&spendkey_bytes),
            bech32::Variant::Bech32,
        )
        .unwrap(),
        &config,
        0,
        false,
    )
    .unwrap();
    regtest_manager.generate_n_blocks(5).unwrap();
    (
        regtest_manager,
        child_process_handler,
        light_client,
        Runtime::new().unwrap(),
    )
}

pub fn two_clients_a_coinbase_backed() -> (
    RegtestManager,
    LightClient,
    LightClient,
    ChildProcessHandler,
    Runtime,
) {
    let (regtest_manager, child_process_handler, client_a, runtime) = coinbasebacked_spendcapable();
    let client_b_zingoconf_path = format!(
        "{}_b",
        regtest_manager.zingo_data_dir.to_string_lossy().to_string()
    );
    std::fs::create_dir(&client_b_zingoconf_path).unwrap();
    let (client_b_config, _height) =
        create_zingoconf_with_datadir(client_a.get_server_uri(), Some(client_b_zingoconf_path))
            .unwrap();
    let client_b = LightClient::new(&client_b_config, 0).unwrap();
    (
        regtest_manager,
        client_a,
        client_b,
        child_process_handler,
        runtime,
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

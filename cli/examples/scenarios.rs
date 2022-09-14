use std::{path::PathBuf, process::Command};

use tokio::runtime::Runtime;
use zingo_cli::regtest::RegtestManager;
use zingoconfig::ZingoConfig;
use zingolib::{create_on_data_dir, lightclient::LightClient};

fn main() {
    send_sapling_to_self()
}
#[test]
fn prove_scenario_is_built() {}

fn send_sapling_to_self() {
    let sapling_key = include_str!("privkey").to_string();
    let mut regtest_manager = RegtestManager::new();
    let mut example_config = PathBuf::from(
        String::from_utf8(Command::new("pwd").output().unwrap().stdout)
            .unwrap()
            .strip_suffix('\n')
            .unwrap(),
    );
    example_config.push("cli");
    example_config.push("examples");
    example_config.push("zcash.conf");
    regtest_manager.zcashd_config = dbg!(example_config);
    let child_process_handler = regtest_manager.launch(true);
    let server = ZingoConfig::get_server_or_default(Some("http://127.0.0.1".to_string()));
    let (config, _height) = create_on_data_dir(server, None).unwrap();
    let client = LightClient::new_from_phrase(sapling_key, &config, 0, false).unwrap();
    regtest_manager.generate_n_blocks(5).unwrap();
    let runtime = Runtime::new().unwrap();

    println!(
        "{}",
        json::stringify_pretty(runtime.block_on(client.do_balance()), 4)
    );
}

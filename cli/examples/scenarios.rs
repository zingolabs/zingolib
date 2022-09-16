#![forbid(unsafe_code)]
use tokio::runtime::Runtime;
use zingo_cli::regtest::{ChildProcessHandler, RegtestManager};
use zingoconfig::ZingoConfig;
use zingolib::{create_on_data_dir, lightclient::LightClient};

fn main() {
    send_mined_sapling_to_orchard()
}
#[test]
fn prove_scenario_is_built() {}

fn setup_scenario_with_imported_mineto_zaddr() -> (RegtestManager, ChildProcessHandler, LightClient)
{
    let sapling_key = include_str!("sapling_regtest_secret_extended_key").to_string();
    let mut regtest_manager = RegtestManager::new();
    let mut example_config = zingo_cli::regtest::get_git_rootdir();
    example_config.push("cli/examples/zcash.conf");
    regtest_manager.zcashd_config = example_config;
    let child_process_handler = regtest_manager.launch(true).unwrap();
    let server = ZingoConfig::get_server_or_default(Some("http://127.0.0.1".to_string()));
    let (config, _height) = create_on_data_dir(
        server,
        Some(regtest_manager.zingo_datadir.to_string_lossy().to_string()),
    )
    .unwrap();
    let client = LightClient::new_from_phrase(sapling_key, &config, 0, false).unwrap();
    (regtest_manager, child_process_handler, client)
}

fn mine_sapling_to_self() {
    let (regtest_manager, _child_process_handler, client) =
        setup_scenario_with_imported_mineto_zaddr();
    regtest_manager.generate_n_blocks(5).unwrap();
    let runtime = Runtime::new().unwrap();

    runtime.block_on(client.do_sync(true)).unwrap();

    let balance = runtime.block_on(client.do_balance());
    assert_eq!(balance["sapling_balance"], 625000000);
}

fn send_mined_sapling_to_orchard() {
    let (regtest_manager, _child_process_handler, client) =
        setup_scenario_with_imported_mineto_zaddr();
    regtest_manager.generate_n_blocks(5).unwrap();
    let runtime = Runtime::new().unwrap();
    runtime.block_on(client.do_sync(true)).unwrap();

    let o_addr = runtime.block_on(client.do_new_address("o")).unwrap()[0].take();
    println!("{o_addr}");
    let send_status = runtime
        .block_on(client.do_send(vec![(
            o_addr.to_string().as_str(),
            5000,
            Some("Scenario test: engage!".to_string()),
        )]))
        .unwrap();

    regtest_manager.generate_n_blocks(2).unwrap();

    let balance = runtime.block_on(client.do_balance());
    assert_eq!(balance["unverified_orchard_balance"], 5000);
    assert_eq!(balance["verified_orchard_balance"], 0);
}

#![forbid(unsafe_code)]
use std::time::Duration;

use tokio::{runtime::Runtime, time::sleep};
use zingo_cli::regtest::{ChildProcessHandler, RegtestManager};
use zingoconfig::ZingoConfig;
use zingolib::{create_zingoconf_with_datadir, lightclient::LightClient};

fn main() {
    send_mined_sapling_to_orchard()
}

#[test]
fn prove_scenario_is_built() {}

fn create_zcash_conf_path(base: &str) -> std::path::PathBuf {
    let mut config = zingo_cli::regtest::get_git_rootdir();
    config.push("cli");
    config.push("examples");
    config.push(base);
    config
}
/// Many scenarios need to start with spendable funds.  This setup provides
/// 1 block worth of coinbase to a preregistered spend capability.
///
/// This key is registered to receive block rewards by:
///  (1) existing accesibly for test code in: cli/examples/mineraddress_sapling_spendingkey
///  (2) corresponding to the address registered as the "mineraddress" field in cli/examples/zcash.conf
fn coinbasebacked_spendcapable_setup() -> (RegtestManager, ChildProcessHandler, LightClient) {
    tracing_subscriber::fmt::init();
    let coinbase_spendkey = include_str!("mineraddress_sapling_spendingkey").to_string();
    let mut regtest_manager = RegtestManager::new();
    regtest_manager.zcashd_config = create_zcash_conf_path("externalwallet_coinbaseaddress.conf");
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

fn mine_sapling_to_self() {
    let (regtest_manager, _child_process_handler, client) = coinbasebacked_spendcapable_setup();
    regtest_manager.generate_n_blocks(5).unwrap();
    let runtime = Runtime::new().unwrap();

    runtime.block_on(client.do_sync(true)).unwrap();

    let balance = runtime.block_on(client.do_balance());
    assert_eq!(balance["sapling_balance"], 625000000);
}

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

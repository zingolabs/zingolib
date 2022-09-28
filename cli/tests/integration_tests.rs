#![forbid(unsafe_code)]
use std::time::Duration;

mod data;
mod setup;
use tokio::time::sleep;

#[test]
fn basic_connectivity_scenario_a() {
    let _ = setup::coinbasebacked_spendcapable();
}
#[test]
fn basic_connectivity_scenario_b() {
    let _ = setup::coinbasebacked_spendcapable();
}
#[test]
fn zcashd_sapling_commitment_tree() {
    let (regtest_manager, _child_process_handler, _client, _runtime) =
        setup::coinbasebacked_spendcapable();
    let trees = regtest_manager
        .get_cli_handle()
        .args(["z_gettreestate", "1"])
        .output()
        .expect("Couldn't get the trees.");
    let trees = json::parse(&String::from_utf8_lossy(&trees.stdout));
    let pretty_trees = json::stringify_pretty(trees.unwrap(), 4);
    println!("{}", pretty_trees);
}

#[test]
fn actual_empty_zcashd_sapling_commitment_tree() {
    // Expectations:
    let sprout_commitments_finalroot =
        "59d2cde5e65c1414c32ba54f0fe4bdb3d67618125286e6a191317917c812c6d7";
    let sapling_commitments_finalroot =
        "3e49b5f954aa9d3545bc6c37744661eea48d7c34e3000d82b7f0010c30f4c2fb";
    let orchard_commitments_finalroot =
        "2fd8e51a03d9bbe2dd809831b1497aeb68a6e37ddf707ced4aa2d8dff13529ae";
    let finalstates = "000000";
    // Setup
    let (regtest_manager, _child_process_handler, _client) = setup::basic_no_spendable();
    // Execution:
    let trees = regtest_manager
        .get_cli_handle()
        .args(["z_gettreestate", "1"])
        .output()
        .expect("Couldn't get the trees.");
    let trees = json::parse(&String::from_utf8_lossy(&trees.stdout));
    // Assertions:
    assert_eq!(
        sprout_commitments_finalroot,
        trees.as_ref().unwrap()["sprout"]["commitments"]["finalRoot"]
    );
    assert_eq!(
        sapling_commitments_finalroot,
        trees.as_ref().unwrap()["sapling"]["commitments"]["finalRoot"]
    );
    assert_eq!(
        orchard_commitments_finalroot,
        trees.as_ref().unwrap()["orchard"]["commitments"]["finalRoot"]
    );
    assert_eq!(
        finalstates,
        trees.as_ref().unwrap()["sprout"]["commitments"]["finalState"]
    );
    assert_eq!(
        finalstates,
        trees.as_ref().unwrap()["sapling"]["commitments"]["finalState"]
    );
    assert_eq!(
        finalstates,
        trees.as_ref().unwrap()["orchard"]["commitments"]["finalState"]
    );
    dbg!(std::process::Command::new("grpcurl").args(["-plaintext", "127.0.0.1:9067"]));
}

#[test]
fn mine_sapling_to_self() {
    let (_regtest_manager, _child_process_handler, client, runtime) =
        setup::coinbasebacked_spendcapable();

    runtime.block_on(async {
        sleep(Duration::from_secs(2)).await;
        client.do_sync(true).await.unwrap();

        let balance = client.do_balance().await;
        println!("{}", json::stringify_pretty(balance.clone(), 4));
        let transactions = client.do_list_transactions(false).await;
        println!("{}", json::stringify_pretty(transactions, 4));
        assert_eq!(balance["sapling_balance"], 3_750_000_000u64);
    });
}

#[test]
fn send_mined_sapling_to_orchard() {
    let (regtest_manager, _child_process_handler, client, runtime) =
        setup::coinbasebacked_spendcapable();
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
        assert_eq!(balance["verified_orchard_balance"], 5000);
    });
    // Proposed Test:
    //#[test]
    //fn two_zcashds_with_colliding_configs() {
    // Expectations:
    //   The children are terminated by the test run end.
    // Setup:
    //   Two different zcashds are configured to launch with 1 config-location
    //todo!("let _regtest_manager = setup::basic_funded_zcashd_lwd_zingolib_connected();");
    // Execution:
    //   Launch the two "bonking" zcashd instances
    // Assertions:
    //   zcashd A is terminated
    //   zcashd B is terminated
    //   The incorrectly configured location is still present (and not checked in)
    //   The test-or-scenario that caused this situation has failed/panicked.
    //}
}

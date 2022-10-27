#![forbid(unsafe_code)]

mod data;
mod utils;
use json::JsonValue;
use tokio::runtime::Runtime;
use utils::setup::{
    basic_no_spendable, saplingcoinbasebacked_spendcapable, two_clients_one_saplingcoinbase_backed,
};
#[test]
fn create_network_disconnected_client() {
    let (_regtest_manager_1, _child_process_handler_1, _client_1) =
        saplingcoinbasebacked_spendcapable();
}

#[test]
fn zcashd_sapling_commitment_tree() {
    let (regtest_manager, _child_process_handler, _client) = saplingcoinbasebacked_spendcapable();
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
    let (regtest_manager, _child_process_handler, _client) = basic_no_spendable();
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
    let (regtest_manager, _child_process_handler, client) = saplingcoinbasebacked_spendcapable();

    Runtime::new().unwrap().block_on(async {
        utils::increase_height_and_sync_client(&regtest_manager, &client, 5).await;

        let balance = client.do_balance().await;
        assert_eq!(balance["sapling_balance"], 3_750_000_000u64);
    });
}

#[test]
fn send_mined_sapling_to_orchard() {
    let (regtest_manager, _child_process_handler, client) = saplingcoinbasebacked_spendcapable();
    Runtime::new().unwrap().block_on(async {
        utils::increase_height_and_sync_client(&regtest_manager, &client, 5).await;

        let o_addr = client.do_new_address("o").await.unwrap()[0].take();
        client
            .do_send(vec![(
                o_addr.to_string().as_str(),
                5000,
                Some("Scenario test: engage!".to_string()),
            )])
            .await
            .unwrap();

        utils::increase_height_and_sync_client(&regtest_manager, &client, 2).await;
        let balance = client.do_balance().await;
        assert_eq!(balance["verified_orchard_balance"], 5000);
    });
}
fn extract_value_as_u64(input: &JsonValue) -> u64 {
    let note = &input["value"].as_fixed_point_u64(0).unwrap();
    note.clone()
}
use zcash_primitives::transaction::components::amount::DEFAULT_FEE;
use zingolib::{create_zingoconf_with_datadir, lightclient::LightClient};
#[test]
fn note_selection_order() {
    //! In order to fund a transaction multiple notes may be selected and consumed.
    //! To minimize note selection operations notes are consumed from largest to smallest.
    //! In addition to testing the order in which notes are selected this test:
    //!   * sends to a sapling address
    //!   * sends back to the original sender's UA
    let (regtest_manager, client_1, client_2, child_process_handler) =
        two_clients_one_saplingcoinbase_backed();

    Runtime::new().unwrap().block_on(async {
        utils::increase_height_and_sync_client(&regtest_manager, &client_1, 5).await;

        // Note that do_addresses returns an array, each element is a JSON representation
        // of a UA.  Legacy addresses can be extracted from the receivers, per:
        // <https://zips.z.cash/zip-0316>
        let client_2_saplingaddress =
            client_2.do_addresses().await[0]["receivers"]["sapling"].clone();

        // Send five transfers in increasing 1000 zat increments
        // These are sent from the coinbase funded client which will
        // subequently receive funding via it's orchard-packed UA.
        for n in 1..=3 {
            client_1
                .do_send(vec![(
                    &client_2_saplingaddress.to_string(),
                    n * 1000,
                    Some(n.to_string()),
                )])
                .await
                .unwrap();
        }
        utils::increase_height_and_sync_client(&regtest_manager, &client_2, 5).await;
        let client_1_unifiedaddress = client_1.do_addresses().await[0]["address"].clone();
        // We know that the largest single note that 2 received from 1 was 3000, for 2 to send
        // 3000 back to 1 it will have to collect funds from two notes to pay the full 3000
        // plus the transaction fee.
        client_2
            .do_send(vec![(
                &client_1_unifiedaddress.to_string(),
                3000,
                Some("Sending back, should have 2 inputs".to_string()),
            )])
            .await
            .unwrap();
        let client_2_notes = client_2.do_list_notes(false).await;
        // The 3000 zat note to cover the value, plus another for the tx-fee.
        let first_value = client_2_notes["pending_sapling_notes"][0]["value"]
            .as_fixed_point_u64(0)
            .unwrap();
        let second_value = client_2_notes["pending_sapling_notes"][1]["value"]
            .as_fixed_point_u64(0)
            .unwrap();
        assert!(
            first_value == 3000u64 && second_value == 2000u64
                || first_value == 2000u64 && second_value == 3000u64
        );
        //);
        // Because the above tx fee won't consume a full note, change will be sent back to 2.
        // This implies that client_2 will have a total of 2 unspent notes:
        //  * one from client_1 sent above (and never used) + 1 as change to itself
        assert_eq!(client_2_notes["unspent_sapling_notes"].len(), 2);
        let change_note = client_2_notes["unspent_sapling_notes"]
            .members()
            .filter(|note| note["is_change"].as_bool().unwrap())
            .collect::<Vec<_>>()[0];
        // Because 2000 is the size of the second largest note.
        assert_eq!(change_note["value"], 2000 - u64::from(DEFAULT_FEE));
        let non_change_note_values = client_2_notes["unspent_sapling_notes"]
            .members()
            .filter(|note| !note["is_change"].as_bool().unwrap())
            .map(|x| extract_value_as_u64(x))
            .collect::<Vec<_>>();
        // client_2 got a total of 3000+2000+1000
        // It sent 3000 to the client_1, and also
        // paid the defualt transaction fee.
        // In non change notes it has 1000.
        // There is an outstanding 2000 that is marked as change.
        // After sync the unspent_sapling_notes should go to 3000.
        assert_eq!(non_change_note_values.iter().sum::<u64>(), 1000u64);

        utils::increase_height_and_sync_client(&regtest_manager, &client_2, 5).await;
        let client_2_post_transaction_notes = client_2.do_list_notes(false).await;
        assert_eq!(
            client_2_post_transaction_notes["pending_sapling_notes"].len(),
            0
        );
        assert_eq!(
            client_2_post_transaction_notes["unspent_sapling_notes"].len(),
            2
        );
        assert_eq!(
            client_2_post_transaction_notes["unspent_sapling_notes"]
                .members()
                .into_iter()
                .map(|x| extract_value_as_u64(x))
                .sum::<u64>(),
            2000u64 // 1000 received and unused + (2000 - 1000 txfee)
        );
    });

    // More explicit than ignoring the unused variable, we only care about this in order to drop it
    drop(child_process_handler);
}

#[test]
fn send_orchard_back_and_forth() {
    let (regtest_manager, client_a, client_b, child_process_handler) =
        two_clients_one_saplingcoinbase_backed();
    Runtime::new().unwrap().block_on(async {
        utils::increase_height_and_sync_client(&regtest_manager, &client_a, 5).await;

        // do_new_address returns a single element json array for some reason
        let ua_of_b = client_b.do_new_address("o").await.unwrap()[0].to_string();
        client_a
            .do_send(vec![(&ua_of_b, 10_000, Some("Orcharding".to_string()))])
            .await
            .unwrap();

        utils::increase_height_and_sync_client(&regtest_manager, &client_b, 3).await;
        client_a.do_sync(true).await.unwrap();

        // We still need to implement sending change to orchard, in librustzcash
        // Even when we do, we'll probably send change to sapling with no
        // preexisting orchard spend authority
        assert_eq!(client_a.do_balance().await["orchard_balance"], 0);
        assert_eq!(client_b.do_balance().await["orchard_balance"], 10_000);

        let ua_of_a = client_a.do_new_address("o").await.unwrap()[0].to_string();
        client_b
            .do_send(vec![(&ua_of_a, 5_000, Some("Sending back".to_string()))])
            .await
            .unwrap();

        utils::increase_height_and_sync_client(&regtest_manager, &client_a, 3).await;
        client_b.do_sync(true).await.unwrap();

        assert_eq!(client_a.do_balance().await["orchard_balance"], 5_000);
        assert_eq!(client_b.do_balance().await["sapling_balance"], 4_000);
        assert_eq!(client_b.do_balance().await["orchard_balance"], 0);

        // Unneeded, but more explicit than having child_process_handler be an
        // unused variable
        drop(child_process_handler);
    });
}

#[test]
fn diversified_addresses_receive_funds_in_best_pool() {
    let (regtest_manager, client_a, client_b, child_process_handler) =
        two_clients_one_saplingcoinbase_backed();
    Runtime::new().unwrap().block_on(async {
        client_b.do_new_address("o").await.unwrap();
        client_b.do_new_address("zo").await.unwrap();
        client_b.do_new_address("z").await.unwrap();

        utils::increase_height_and_sync_client(&regtest_manager, &client_a, 5).await;
        let addresses = client_b.do_addresses().await;
        let address_5000_nonememo_tuples = addresses
            .members()
            .map(|ua| (ua["address"].as_str().unwrap(), 5_000, None))
            .collect::<Vec<(&str, u64, Option<String>)>>();
        client_a
            .do_send(address_5000_nonememo_tuples)
            .await
            .unwrap();
        utils::increase_height_and_sync_client(&regtest_manager, &client_b, 5).await;
        let balance_b = client_b.do_balance().await;
        assert_eq!(
            balance_b,
            json::object! {
                "sapling_balance": 5000,
                "verified_sapling_balance": 5000,
                "spendable_sapling_balance": 5000,
                "unverified_sapling_balance": 0,
                "orchard_balance": 15000,
                "verified_orchard_balance": 15000,
                "spendable_orchard_balance": 15000,
                "unverified_orchard_balance": 0,
                "transparent_balance": 0
            }
        );
        // Unneeded, but more explicit than having child_process_handler be an
        // unused variable
        drop(child_process_handler);
    });
}

#[test]
fn rescan_still_have_outgoing_metadata() {
    let (regtest_manager, client_one, client_two, child_process_handler) =
        two_clients_one_saplingcoinbase_backed();
    Runtime::new().unwrap().block_on(async {
        utils::increase_height_and_sync_client(&regtest_manager, &client_one, 5).await;
        let sapling_addr_of_two = client_two.do_new_address("tz").await.unwrap();
        client_one
            .do_send(vec![(
                sapling_addr_of_two[0].as_str().unwrap(),
                1_000,
                Some("foo".to_string()),
            )])
            .await
            .unwrap();
        utils::increase_height_and_sync_client(&regtest_manager, &client_one, 5).await;
        let transactions = client_one.do_list_transactions(false).await;
        client_one.do_rescan().await.unwrap();
        let post_rescan_transactions = client_one.do_list_transactions(false).await;
        assert_eq!(transactions, post_rescan_transactions);

        drop(child_process_handler);
    });
}

///
#[test]
fn rescan_still_have_outgoing_metadata_with_sends_to_self() {
    let (regtest_manager, child_process_handler, client) = saplingcoinbasebacked_spendcapable();
    Runtime::new().unwrap().block_on(async {
        utils::increase_height_and_sync_client(&regtest_manager, &client, 5).await;
        let sapling_addr = client.do_new_address("tz").await.unwrap();
        for memo in [None, Some("foo")] {
            client
                .do_send(vec![(
                    sapling_addr[0].as_str().unwrap(),
                    client.do_balance().await["spendable_sapling_balance"]
                        .as_u64()
                        .unwrap()
                        - 1_000,
                    memo.map(ToString::to_string),
                )])
                .await
                .unwrap();
            utils::increase_height_and_sync_client(&regtest_manager, &client, 5).await;
        }
        let transactions = client.do_list_transactions(false).await;
        let notes = client.do_list_notes(true).await;
        client.do_rescan().await.unwrap();
        let post_rescan_transactions = client.do_list_transactions(false).await;
        let post_rescan_notes = client.do_list_notes(true).await;
        assert_eq!(transactions, post_rescan_transactions);

        // Notes are not in deterministic order after rescan. Insead, iterate over all
        // the notes and check that they exist post-rescan
        for (field_name, field) in notes.entries() {
            for note in field.members() {
                assert!(post_rescan_notes[field_name]
                    .members()
                    .any(|post_rescan_note| post_rescan_note == note));
            }
            assert_eq!(field.len(), post_rescan_notes[field_name].len());
        }
        drop(child_process_handler);
    });
}

#[test]
fn handling_of_nonregenerated_diversified_addresses_after_seed_restore() {
    let (regtest_manager, client_a, client_b, child_process_handler) =
        two_clients_one_saplingcoinbase_backed();
    let mut expected_unspent_sapling_notes = json::object! {

            "created_in_block" =>  7,
            "datetime" =>  1666631643,
            "created_in_txid" => "4eeaca8d292f07f9cbe26a276f7658e75f0ef956fb21646e3907e912c5af1ec5",
            "value" =>  5_000,
            "unconfirmed" =>  false,
            "is_change" =>  false,
            "address" =>  "uregtest1gvnz2m8wkzxmdvzc7w6txnaam8d7k8zx7zdn0dlglyud5wltsc9d2xf26ptz79d399esc66f5lkmvg95jpa7c5sqt7hgtnvp4xxkypew8w9weuqa8wevy85nz4yr8u508ekw5qwxff8",
            "spendable" =>  true,
            "spent" =>  JsonValue::Null,
            "spent_at_height" =>  JsonValue::Null,
            "unconfirmed_spent" =>  JsonValue::Null,

    };
    let seed = Runtime::new().unwrap().block_on(async {
        utils::increase_height_and_sync_client(&regtest_manager, &client_a, 5).await;
        let sapling_addr_of_b = client_b.do_new_address("tz").await.unwrap();
        client_a
            .do_send(vec![(
                sapling_addr_of_b[0].as_str().unwrap(),
                5_000,
                Some("foo".to_string()),
            )])
            .await
            .unwrap();
        utils::increase_height_and_sync_client(&regtest_manager, &client_a, 5).await;
        client_b.do_sync(true).await.unwrap();
        let notes = client_b.do_list_notes(true).await;
        assert_eq!(notes["unspent_sapling_notes"].members().len(), 1);
        let note = notes["unspent_sapling_notes"].members().next().unwrap();
        //The following fields aren't known until runtime, and should be cryptographically nondeterministic
        //Testing that they're generated correctly is beyond the scope if this test
        expected_unspent_sapling_notes["datetime"] = note["datetime"].clone();
        expected_unspent_sapling_notes["created_in_txid"] = note["created_in_txid"].clone();

        assert_eq!(
            note,
            &expected_unspent_sapling_notes,
            "Expected: {}\nActual: {}",
            json::stringify_pretty(expected_unspent_sapling_notes.clone(), 4),
            json::stringify_pretty(note.clone(), 4)
        );
        client_b.do_seed_phrase().await.unwrap()
    });
    let (config, _height) = create_zingoconf_with_datadir(
        client_b.get_server_uri(),
        regtest_manager
            .zingo_data_dir
            .to_str()
            .map(ToString::to_string),
    )
    .unwrap();
    let mut expected_unspent_sapling_notes_after_restore_from_seed =
        expected_unspent_sapling_notes.clone();
    expected_unspent_sapling_notes_after_restore_from_seed["address"] = JsonValue::String(
        "Diversifier not in wallet. Perhaps you restored from seed and didn't restore addresses"
            .to_string(),
    );
    let client_b_restored = LightClient::create_with_seedorkey_wallet(
        seed["seed"].as_str().unwrap().to_string(),
        &config,
        0,
        true,
    )
    .unwrap();
    Runtime::new().unwrap().block_on(async {
        client_b_restored.do_sync(true).await.unwrap();
        let notes = client_b_restored.do_list_notes(true).await;
        assert_eq!(notes["unspent_sapling_notes"].members().len(), 1);
        let note = notes["unspent_sapling_notes"].members().next().unwrap();
        assert_eq!(
            note,
            &expected_unspent_sapling_notes_after_restore_from_seed,
            "Expected: {}\nActual: {}",
            json::stringify_pretty(
                expected_unspent_sapling_notes_after_restore_from_seed.clone(),
                4
            ),
            json::stringify_pretty(note.clone(), 4)
        );

        //The first address in a wallet should always contain all three currently existant receiver types
        let address_of_a = &client_a.do_addresses().await[0]["address"];
        client_b_restored
            .do_send(vec![(address_of_a.as_str().unwrap(), 4_000, None)])
            .await
            .unwrap();
        utils::increase_height_and_sync_client(&regtest_manager, &client_a, 5).await;

        //Ensure that client_b_restored was still able to spend the note, despite not having the
        //diversified address associated with it
        assert_eq!(
            client_a.do_balance().await["spendable_orchard_balance"],
            4_000
        );
    });
    drop(child_process_handler);
}

#[test]
fn ensure_taddrs_from_old_seeds_work() {
    let (regtest_manager, child_process_handler, client_a) = saplingcoinbasebacked_spendcapable();

    let client_b_zingoconf_path = format!(
        "{}_two",
        regtest_manager.zingo_data_dir.to_string_lossy().to_string()
    );
    std::fs::create_dir(&client_b_zingoconf_path).unwrap();
    let (client_b_config, _height) =
        create_zingoconf_with_datadir(client_a.get_server_uri(), Some(client_b_zingoconf_path))
            .unwrap();

    // The first four taddrs generated on commit 9e71a14eb424631372fd08503b1bd83ea763c7fb
    // Generated from the following seed
    let transparent_addresses = [
        "tmFLszfkjgim4zoUMAXpuohnFBAKy99rr2i",
        "tmAtLC3JkTDrXyn5okUbb6qcMGE4Xq4UdhD",
        "tmDSApneNXLWcw1unFCvJEus3Ugnpw2fPLy",
        "tmU29L8gXXmSpRcHKE2GLFLRW4suQ95opci",
    ];
    let _normalized_index_addresses = [
        "tmMKJvgXLgckRbL2qArhKmLSmvhMEXgtTAc",
        "tmFDjcf9T35kzLAVwGSeCiFRe6XDNMpBQo9",
        "tmKxoeD2pbmpcac5RBFw9Rvc7h5KN1ZJ83o",
        "tmEceryatGjB3sY9xPRdXCbXiTBXGEUTFbm",
    ];
    let _hardened_index_addresses = [
        "tmGwfiiLDUSVxsWAY66UP76r1E68YTt64d1",
        "tmSwk8bjXdCgBvpS8Kybk5nUyE21QFcDqre",
        "tmEMgi2cSnEWmRGbgGxFiSsGF5a4gPATNWA",
        "tmTtvtJvGaBATERDFZGQhvaRxDS9KCHUf9r",
    ];
    let seed = "hospital museum valve antique skate museum \
    unfold vocal weird milk scale social vessel identify \
    crowd hospital control album rib bulb path oven civil tank";
    let client_b =
        LightClient::create_with_seedorkey_wallet(seed.to_string(), &client_b_config, 0, false)
            .unwrap();

    Runtime::new().unwrap().block_on(async {
        for _ in 0..3 {
            client_b.do_new_address("tzo").await.unwrap();
        }
        let addresses = client_b.do_addresses().await;
        println!("{}", json::stringify_pretty(addresses.clone(), 4));
        for (i, address) in addresses.members().enumerate() {
            assert_eq!(
                address["receivers"]["transparent"].to_string(),
                transparent_addresses[i]
            )
        }
    });
    drop(child_process_handler);
}

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

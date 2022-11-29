#![forbid(unsafe_code)]

mod data;
mod utils;
use data::TEST_SEED;
use json::JsonValue;
use tokio::runtime::Runtime;
use utils::setup;
#[test]
fn create_network_disconnected_client() {
    let (_regtest_manager_1, _child_process_handler_1, _client_builder) =
        setup::saplingcoinbasebacked_spendcapable();
}

#[test]
fn zcashd_sapling_commitment_tree() {
    let (regtest_manager, _child_process_handler, _client_builder) =
        setup::saplingcoinbasebacked_spendcapable();
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
fn verify_old_wallet_uses_server_height_in_send() {
    let (regtest_manager, child_process_handler, mut client_builder) =
        saplingcoinbasebacked_spendcapable();
    let client_sending = client_builder.new_sameseed_client(0, false);
    let client_receiving = client_builder.new_plantedseed_client(TEST_SEED.to_string(), 0, false);
    Runtime::new().unwrap().block_on(async {
        // Ensure that the client has confirmed spendable funds
        utils::increase_height_and_sync_client(&regtest_manager, &client_sending, 5).await;

        // Without sync push server forward 100 blocks
        utils::increase_server_height(&regtest_manager, 100).await;
        let ua = client_receiving.do_new_address("o").await.unwrap()[0].to_string();
        let client_wallet_height = client_sending.do_wallet_last_scanned_height().await;

        // Verify that wallet is still back at 6.
        assert_eq!(client_wallet_height, 6);

        // Interrupt generating send
        client_sending
            .do_send(vec![(&ua, 10_000, Some("Interrupting sync!!".to_string()))])
            .await
            .unwrap();
    });
    drop(child_process_handler);
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
    let (regtest_manager, _child_process_handler, mut client_builder) =
        setup::saplingcoinbasebacked_spendcapable();
    let client = client_builder.new_sameseed_client(0, false);
    Runtime::new().unwrap().block_on(async {
        utils::increase_height_and_sync_client(&regtest_manager, &client, 5).await;

        let balance = client.do_balance().await;
        assert_eq!(balance["sapling_balance"], 3_750_000_000u64);
    });
}

#[test]
fn send_mined_sapling_to_orchard() {
    //! This test shows the 5th confirmation changing the state of balance by
    //! debiting unverified_orchard_balance and crediting verified_orchard_balance.  The debit amount is
    //! consistent with all the notes in the relevant block changing state.
    //! NOTE that the balance doesn't give insight into the distribution across notes.
    let (regtest_manager, _child_process_handler, mut client_builder) =
        setup::saplingcoinbasebacked_spendcapable();
    let client = client_builder.new_sameseed_client(0, false);
    Runtime::new().unwrap().block_on(async {
        utils::increase_height_and_sync_client(&regtest_manager, &client, 5).await;

        let o_addr = client.do_new_address("o").await.unwrap()[0].take();
        let amount_to_send = 5_000;
        client
            .do_send(vec![(
                o_addr.to_string().as_str(),
                amount_to_send,
                Some("Scenario test: engage!".to_string()),
            )])
            .await
            .unwrap();

        utils::increase_height_and_sync_client(&regtest_manager, &client, 4).await;
        let balance = client.do_balance().await;
        // We send change to orchard now, so we should have the full value of the note
        // we spent, minus the transaction fee
        assert_eq!(
            balance["unverified_orchard_balance"],
            625_000_000 - u64::from(DEFAULT_FEE)
        );
        assert_eq!(balance["verified_orchard_balance"], 0);
        utils::increase_height_and_sync_client(&regtest_manager, &client, 1).await;
        let balance = client.do_balance().await;
        assert_eq!(balance["unverified_orchard_balance"], 0);
        assert_eq!(
            balance["verified_orchard_balance"],
            625_000_000 - u64::from(DEFAULT_FEE)
        );
    });
}
fn extract_value_as_u64(input: &JsonValue) -> u64 {
    let note = &input["value"].as_fixed_point_u64(0).unwrap();
    note.clone()
}
use zcash_primitives::transaction::components::amount::DEFAULT_FEE;
#[test]
fn note_selection_order() {
    //! In order to fund a transaction multiple notes may be selected and consumed.
    //! To minimize note selection operations notes are consumed from largest to smallest.
    //! In addition to testing the order in which notes are selected this test:
    //!   * sends to a sapling address
    //!   * sends back to the original sender's UA
    let (regtest_manager, client_1, client_2, child_process_handler, _) =
        setup::two_clients_one_saplingcoinbase_backed();

    Runtime::new().unwrap().block_on(async {
        utils::increase_height_and_sync_client(&regtest_manager, &client_1, 5).await;

        // Note that do_addresses returns an array, each element is a JSON representation
        // of a UA.  Legacy addresses can be extracted from the receivers, per:
        // <https://zips.z.cash/zip-0316>
        let client_2_saplingaddress = client_2.do_addresses().await[0]["receivers"]["sapling"]
            .clone()
            .to_string();

        // Send three transfers in increasing 1000 zat increments
        // These are sent from the coinbase funded client which will
        // subequently receive funding via it's orchard-packed UA.
        client_1
            .do_send(
                (1..=3)
                    .map(|n| {
                        (
                            client_2_saplingaddress.as_str(),
                            n * 1000,
                            Some(n.to_string()),
                        )
                    })
                    .collect(),
            )
            .await
            .unwrap();

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
        //  * one (sapling) from client_1 sent above (and never used) + 1 (orchard) as change to itself
        assert_eq!(client_2_notes["unspent_sapling_notes"].len(), 1);
        assert_eq!(client_2_notes["unspent_orchard_notes"].len(), 1);
        let change_note = client_2_notes["unspent_orchard_notes"]
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
            1
        );
        assert_eq!(
            client_2_post_transaction_notes["unspent_orchard_notes"].len(),
            1
        );
        assert_eq!(
            client_2_post_transaction_notes["unspent_sapling_notes"]
                .members()
                .into_iter()
                .chain(
                    client_2_post_transaction_notes["unspent_orchard_notes"]
                        .members()
                        .into_iter()
                )
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
    let (regtest_manager, client_a, client_b, child_process_handler, _) =
        setup::two_clients_one_saplingcoinbase_backed();
    Runtime::new().unwrap().block_on(async {
        utils::increase_height_and_sync_client(&regtest_manager, &client_a, 5).await;

        let ua_of_b = client_b.do_addresses().await[0]["address"].to_string();
        client_a
            .do_send(vec![(&ua_of_b, 10_000, Some("Orcharding".to_string()))])
            .await
            .unwrap();

        utils::increase_height_and_sync_client(&regtest_manager, &client_b, 5).await;
        client_a.do_sync(true).await.unwrap();

        // Regtest is generating notes with a block reward of 625_000_000 zats.
        assert_eq!(
            client_a.do_balance().await["orchard_balance"],
            625_000_000 - 10_000 - u64::from(DEFAULT_FEE)
        );
        assert_eq!(client_b.do_balance().await["orchard_balance"], 10_000);

        let ua_of_a = client_a.do_addresses().await[0]["address"].to_string();
        client_b
            .do_send(vec![(&ua_of_a, 5_000, Some("Sending back".to_string()))])
            .await
            .unwrap();

        utils::increase_height_and_sync_client(&regtest_manager, &client_a, 3).await;
        client_b.do_sync(true).await.unwrap();

        assert_eq!(
            client_a.do_balance().await["orchard_balance"],
            625_000_000 - 10_000 - u64::from(DEFAULT_FEE) + 5_000
        );
        assert_eq!(client_b.do_balance().await["sapling_balance"], 0);
        assert_eq!(client_b.do_balance().await["orchard_balance"], 4_000);
        println!(
            "{}",
            json::stringify_pretty(client_a.do_list_transactions(false).await, 4)
        );

        // Unneeded, but more explicit than having child_process_handler be an
        // unused variable
        drop(child_process_handler);
    });
}

#[test]
fn diversified_addresses_receive_funds_in_best_pool() {
    let (regtest_manager, client_a, client_b, child_process_handler, _) =
        setup::two_clients_one_saplingcoinbase_backed();
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
    let (regtest_manager, client_one, client_two, child_process_handler, _) =
        setup::two_clients_one_saplingcoinbase_backed();
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
    let (regtest_manager, child_process_handler, mut client_builder) =
        setup::saplingcoinbasebacked_spendcapable();
    let client = client_builder.new_sameseed_client(0, false);
    Runtime::new().unwrap().block_on(async {
        utils::increase_height_and_sync_client(&regtest_manager, &client, 5).await;
        let sapling_addr = client.do_new_address("tz").await.unwrap();
        for memo in [None, Some("foo")] {
            client
                .do_send(vec![(
                    sapling_addr[0].as_str().unwrap(),
                    {
                        let balance = client.do_balance().await;
                        balance["spendable_sapling_balance"].as_u64().unwrap()
                            + balance["spendable_orchard_balance"].as_u64().unwrap()
                    } - 1_000,
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
        assert_eq!(
            transactions,
            post_rescan_transactions,
            "Pre-Rescan: {}\n\n\nPost-Rescan: {}",
            json::stringify_pretty(transactions.clone(), 4),
            json::stringify_pretty(post_rescan_transactions.clone(), 4)
        );

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

/// An arbitrary number of diversified addresses may be generated
/// from a seed.  If the wallet is subsequently lost-or-destroyed
/// wallet-regeneration-from-seed (sprouting) doesn't regenerate
/// the previous diversifier list.
#[test]
fn handling_of_nonregenerated_diversified_addresses_after_seed_restore() {
    let (regtest_manager, sender, recipient, child_process_handler, mut client_builder) =
        setup::two_clients_one_saplingcoinbase_backed();
    let mut expected_unspent_sapling_notes = json::object! {

            "created_in_block" =>  7,
            "datetime" =>  1666631643,
            "created_in_txid" => "4eeaca8d292f07f9cbe26a276f7658e75f0ef956fb21646e3907e912c5af1ec5",
            "value" =>  5_000,
            "unconfirmed" =>  false,
            "is_change" =>  false,
            "address" =>  "uregtest1m8un60udl5ac0928aghy4jx6wp59ty7ct4t8ks9udwn8y6fkdmhe6pq0x5huv8v0pprdlq07tclqgl5fzfvvzjf4fatk8cpyktaudmhvjcqufdsfmktgawvne3ksrhs97pf0u8s8f8h",
            "spendable" =>  true,
            "spent" =>  JsonValue::Null,
            "spent_at_height" =>  JsonValue::Null,
            "unconfirmed_spent" =>  JsonValue::Null,

    };
    let original_recipient_address = "\
        uregtest1qtqr46fwkhmdn336uuyvvxyrv0l7trgc0z9clpryx6vtladnpyt4wvq99p59f4rcyuvpmmd0hm4k5vv6j\
        8edj6n8ltk45sdkptlk7rtzlm4uup4laq8ka8vtxzqemj3yhk6hqhuypupzryhv66w65lah9ms03xa8nref7gux2zz\
        hjnfanxnnrnwscmz6szv2ghrurhu3jsqdx25y2yh";
    let seed_of_recipient = Runtime::new().unwrap().block_on(async {
        utils::increase_height_and_sync_client(&regtest_manager, &sender, 5).await;
        let addresses = recipient.do_addresses().await;
        assert_eq!(&addresses[0]["address"], &original_recipient_address);
        let recipient_addr = recipient.do_new_address("tz").await.unwrap();
        sender
            .do_send(vec![(
                recipient_addr[0].as_str().unwrap(),
                5_000,
                Some("foo".to_string()),
            )])
            .await
            .unwrap();
        utils::increase_height_and_sync_client(&regtest_manager, &sender, 5).await;
        recipient.do_sync(true).await.unwrap();
        let notes = recipient.do_list_notes(true).await;
        assert_eq!(notes["unspent_sapling_notes"].members().len(), 1);
        let note = notes["unspent_sapling_notes"].members().next().unwrap();
        //The following fields aren't known until runtime, and should be cryptographically nondeterministic
        //Testing that they're generated correctly is beyond the scope if this test
        expected_unspent_sapling_notes["datetime"] = note["datetime"].clone();
        expected_unspent_sapling_notes["created_in_txid"] = note["created_in_txid"].clone();

        assert_eq!(
            note,
            &expected_unspent_sapling_notes,
            "\nExpected:\n{}\n===\nActual:\n{}\n",
            json::stringify_pretty(expected_unspent_sapling_notes.clone(), 4),
            json::stringify_pretty(note.clone(), 4)
        );
        recipient.do_seed_phrase().await.unwrap()
    });
    drop(recipient); // Discard original to ensure subsequent data is fresh.
    let mut expected_unspent_sapling_notes_after_restore_from_seed =
        expected_unspent_sapling_notes.clone();
    expected_unspent_sapling_notes_after_restore_from_seed["address"] = JsonValue::String(
        "Diversifier not in wallet. Perhaps you restored from seed and didn't restore addresses"
            .to_string(),
    );
    let recipient_restored = client_builder.new_plantedseed_client(
        seed_of_recipient["seed"].as_str().unwrap().to_string(),
        0,
        true,
    );
    let seed_of_recipient_restored = Runtime::new().unwrap().block_on(async {
        recipient_restored.do_sync(true).await.unwrap();
        let restored_addresses = recipient_restored.do_addresses().await;
        assert_eq!(
            &restored_addresses[0]["address"],
            &original_recipient_address
        );
        let notes = recipient_restored.do_list_notes(true).await;
        assert_eq!(notes["unspent_sapling_notes"].members().len(), 1);
        let note = notes["unspent_sapling_notes"].members().next().unwrap();
        assert_eq!(
            note,
            &expected_unspent_sapling_notes_after_restore_from_seed,
            "\nExpected:\n{}\n===\nActual:\n{}\n",
            json::stringify_pretty(
                expected_unspent_sapling_notes_after_restore_from_seed.clone(),
                4
            ),
            json::stringify_pretty(note.clone(), 4)
        );

        //The first address in a wallet should always contain all three currently extant
        //receiver types.
        let sender_address = &sender.do_addresses().await[0]["address"];
        recipient_restored
            .do_send(vec![(sender_address.as_str().unwrap(), 4_000, None)])
            .await
            .unwrap();
        let sender_balance = sender.do_balance().await;
        utils::increase_height_and_sync_client(&regtest_manager, &sender, 5).await;

        //Ensure that recipient_restored was still able to spend the note, despite not having the
        //diversified address associated with it
        assert_eq!(
            sender.do_balance().await["spendable_orchard_balance"],
            sender_balance["spendable_orchard_balance"]
                .as_u64()
                .unwrap()
                + 4_000
        );
        recipient_restored.do_seed_phrase().await.unwrap()
    });
    assert_eq!(seed_of_recipient, seed_of_recipient_restored);
    drop(child_process_handler);
}

#[test]
fn ensure_taddrs_from_old_seeds_work() {
    let (_regtest_manager, child_process_handler, mut client_builder) =
        setup::saplingcoinbasebacked_spendcapable();
    // The first taddr generated on commit 9e71a14eb424631372fd08503b1bd83ea763c7fb
    let transparent_address = "tmFLszfkjgim4zoUMAXpuohnFBAKy99rr2i";

    let client_b = client_builder.new_plantedseed_client(TEST_SEED.to_string(), 0, false);

    Runtime::new().unwrap().block_on(async {
        client_b.do_new_address("zt").await.unwrap();
        let addresses = client_b.do_addresses().await;
        println!("{}", json::stringify_pretty(addresses.clone(), 4));
        assert_eq!(
            addresses[0]["receivers"]["transparent"].to_string(),
            transparent_address
        )
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

#[cfg(feature = "cross_version")]
#[test]
fn cross_compat() {
    let (_regtest_manager, current_client, fixed_taddr_client, child_process_handler) =
        setup::cross_version_setup();

    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let fixed_taddr_seed = fixed_taddr_client.do_seed_phrase().await.unwrap();
        let current_seed = current_client.do_seed_phrase().await.unwrap();
        assert_eq!(fixed_taddr_seed["seed"], current_seed["seed"]);
        let fixed_taddresses = fixed_taddr_client.do_addresses().await;
        let current_addresses = fixed_taddr_client.do_addresses().await;
        assert_eq!(fixed_taddresses, current_addresses);
    });
    drop(child_process_handler);
}

#[test]
fn t_incoming_t_outgoing() {
    let (regtest_manager, sender, recipient, child_process_handler, _client_builder) =
        setup::two_clients_one_saplingcoinbase_backed();

    tokio::runtime::Runtime::new().unwrap().block_on(async {
        utils::increase_height_and_sync_client(&regtest_manager, &sender, 9).await;
        // 2. Get an incoming transaction to a t address
        let taddr = recipient.do_addresses().await[0]["receivers"]["transparent"].clone();
        let value = 100_000;

        sender
            .do_send(vec![(taddr.as_str().unwrap(), value, None)])
            .await
            .unwrap();

        recipient.do_sync(true).await.unwrap();
        utils::increase_height_and_sync_client(&regtest_manager, &recipient, 1).await;

        // 3. Test the list
        let list = recipient.do_list_transactions(false).await;
        assert_eq!(list[0]["block_height"].as_u64().unwrap(), 11);
        assert_eq!(list[0]["address"], taddr);
        assert_eq!(list[0]["amount"].as_u64().unwrap(), value);

        // 4. We can spend the funds immediately, since this is a taddr
        let sent_value = 20_000;
        let sent_transaction_id = recipient
            .do_send(vec![(EXT_TADDR, sent_value, None)])
            .await
            .unwrap();
        utils::increase_height_and_sync_client(&regtest_manager, &recipient, 1).await;

        // 5. Test the unconfirmed send.
        let list = recipient.do_list_transactions(false).await;
        assert_eq!(list[1]["block_height"].as_u64().unwrap(), 12);
        assert_eq!(list[1]["txid"], sent_transaction_id);
        assert_eq!(
            list[1]["amount"].as_i64().unwrap(),
            -(sent_value as i64 + i64::from(DEFAULT_FEE))
        );
        assert_eq!(list[1]["unconfirmed"].as_bool().unwrap(), false);
        assert_eq!(list[1]["outgoing_metadata"][0]["address"], EXT_TADDR);
        assert_eq!(
            list[1]["outgoing_metadata"][0]["value"].as_u64().unwrap(),
            sent_value
        );

        let notes = recipient.do_list_notes(true).await;
        assert_eq!(
            notes["spent_utxos"][0]["created_in_block"]
                .as_u64()
                .unwrap(),
            11
        );
        assert_eq!(
            notes["spent_utxos"][0]["spent_at_height"].as_u64().unwrap(),
            12
        );
        assert_eq!(notes["spent_utxos"][0]["spent"], sent_transaction_id);

        // Change shielded note
        assert_eq!(
            notes["unspent_orchard_notes"][0]["created_in_block"]
                .as_u64()
                .unwrap(),
            12
        );
        assert_eq!(
            notes["unspent_orchard_notes"][0]["created_in_txid"],
            sent_transaction_id
        );
        assert_eq!(
            notes["unspent_orchard_notes"][0]["is_change"]
                .as_bool()
                .unwrap(),
            true
        );
        assert_eq!(
            notes["unspent_orchard_notes"][0]["value"].as_u64().unwrap(),
            value - sent_value - u64::from(DEFAULT_FEE)
        );

        let list = recipient.do_list_transactions(false).await;
        println!("{}", json::stringify_pretty(list.clone(), 4));

        assert_eq!(list[1]["block_height"].as_u64().unwrap(), 12);
        assert_eq!(list[1]["txid"], sent_transaction_id);
        assert_eq!(list[1]["unconfirmed"].as_bool().unwrap(), false);
        assert_eq!(list[1]["outgoing_metadata"][0]["address"], EXT_TADDR);
        assert_eq!(
            list[1]["outgoing_metadata"][0]["value"].as_u64().unwrap(),
            sent_value
        );

        // Make sure everything is fine even after the rescan

        recipient.do_rescan().await.unwrap();

        let list = recipient.do_list_transactions(false).await;
        println!("{}", json::stringify_pretty(list.clone(), 4));
        assert_eq!(list[1]["block_height"].as_u64().unwrap(), 12);
        assert_eq!(list[1]["txid"], sent_transaction_id);
        assert_eq!(list[1]["unconfirmed"].as_bool().unwrap(), false);
        assert_eq!(list[1]["outgoing_metadata"][0]["address"], EXT_TADDR);
        assert_eq!(
            list[1]["outgoing_metadata"][0]["value"].as_u64().unwrap(),
            sent_value
        );

        let notes = recipient.do_list_notes(true).await;
        // Change shielded note
        assert_eq!(
            notes["unspent_orchard_notes"][0]["created_in_block"]
                .as_u64()
                .unwrap(),
            12
        );
        assert_eq!(
            notes["unspent_orchard_notes"][0]["created_in_txid"],
            sent_transaction_id
        );
        assert_eq!(
            notes["unspent_orchard_notes"][0]["is_change"]
                .as_bool()
                .unwrap(),
            true
        );
        assert_eq!(
            notes["unspent_orchard_notes"][0]["value"].as_u64().unwrap(),
            value - sent_value - u64::from(DEFAULT_FEE)
        );
    });
    drop(child_process_handler);
}

#[test]
fn send_to_ua_saves_full_ua_in_wallet() {
    let (regtest_manager, sender, recipient, child_process_handler, _client_builder) =
        setup::two_clients_one_saplingcoinbase_backed();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        utils::increase_height_and_sync_client(&regtest_manager, &sender, 5).await;
        let recipient_address = recipient.do_addresses().await[0]["address"].take();
        let sent_value = 50_000;
        sender
            .do_send(vec![(
                recipient_address.as_str().unwrap(),
                sent_value,
                None,
            )])
            .await
            .unwrap();
        utils::increase_height_and_sync_client(&regtest_manager, &sender, 3).await;
        let list = sender.do_list_transactions(false).await;
        assert!(list.members().any(|transaction| {
            transaction.entries().any(|(key, value)| {
                if key == "outgoing_metadata" {
                    value[0]["address"] == recipient_address
                } else {
                    false
                }
            })
        }));
        sender.do_rescan().await.unwrap();
        let new_list = sender.do_list_transactions(false).await;
        assert!(new_list.members().any(|transaction| {
            transaction.entries().any(|(key, value)| {
                if key == "outgoing_metadata" {
                    value[0]["address"] == recipient_address
                } else {
                    false
                }
            })
        }));
        assert_eq!(
            list,
            new_list,
            "Pre-Rescan: {}\n\n\nPost-Rescan: {}\n\n\n",
            json::stringify_pretty(list.clone(), 4),
            json::stringify_pretty(new_list.clone(), 4)
        );
    });
    drop(child_process_handler);
}

// Burn-to regtest address generated by `zcash-cli getnewaddress`
const EXT_TADDR: &str = "tmJTBtMwPU96XteSiP89xDz1WARNgRddEHq";

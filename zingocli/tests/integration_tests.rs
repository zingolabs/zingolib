#![forbid(unsafe_code)]
#![cfg(feature = "local_env")]
mod data;
mod utils;
use std::fs::File;

use bip0039::Mnemonic;
use data::seeds::HOSPITAL_MUSEUM_SEED;
use json::JsonValue;
use utils::scenarios;

use zcash_address::unified::Ufvk;
use zcash_primitives::transaction::components::amount::DEFAULT_FEE;
use zingoconfig::{ChainType, ZingoConfig};
use zingolib::{
    check_client_balances, get_base_address,
    lightclient::LightClient,
    wallet::{
        keys::{
            extended_transparent::ExtendedPrivKey,
            unified::{Capability, WalletCapability},
        },
        LightWallet, WalletBase,
    },
};

#[tokio::test]
async fn factor_do_shield_to_call_do_send() {
    let (regtest_manager, _child_process_handler, faucet, recipient) =
        scenarios::faucet_recipient().await;
    utils::increase_height_and_sync_client(&regtest_manager, &faucet, 2).await;
    faucet
        .do_send(vec![(
            &get_base_address!(recipient, "transparent"),
            1_000u64,
            None,
        )])
        .await
        .unwrap();
}

#[tokio::test]
async fn test_scanning_in_watch_only_mode() {
    // # Scenario:
    // 3. reset wallet
    // 4. for every combination of FVKs
    //     4.1. init a wallet with UFVK
    //     4.2. check that the wallet is empty
    //     4.3. rescan
    //     4.4. check that notes and utxos were detected by the wallet
    //
    // # Current watch-only mode limitations:
    // - wallet will not detect funds on all transparent addresses
    //   see: https://github.com/zingolabs/zingolib/issues/245
    // - wallet will not detect funds on internal addresses
    //   see: https://github.com/zingolabs/zingolib/issues/246

    let (regtest_manager, child_process_handler, mut client_builder) = scenarios::custom_clients();
    let faucet = client_builder.build_new_faucet(0, false).await;
    let original_recipient = client_builder
        .build_newseed_client(HOSPITAL_MUSEUM_SEED.to_string(), 0, false)
        .await;
    let (zingo_config, _) = zingolib::load_clientconfig_async(
        client_builder.server_id,
        Some(client_builder.zingo_datadir),
    )
    .await
    .unwrap();

    let (recipient_taddr, recipient_sapling, recipient_unified) = (
        get_base_address!(original_recipient, "transparent"),
        get_base_address!(original_recipient, "sapling"),
        get_base_address!(original_recipient, "unified"),
    );
    let addr_amount_memos = vec![
        (recipient_taddr.as_str(), 1_000u64, None),
        (recipient_sapling.as_str(), 2_000u64, None),
        (recipient_unified.as_str(), 3_000u64, None),
    ];
    // 1. fill wallet with a coinbase transaction by syncing faucet with 1-block increase
    utils::increase_height_and_sync_client(&regtest_manager, &faucet, 1).await;
    // 2. send a transaction contaning all types of outputs
    faucet.do_send(addr_amount_memos).await.unwrap();
    utils::increase_height_and_sync_client(&regtest_manager, &original_recipient, 1).await;
    let original_recipient_balance = original_recipient.do_balance().await;
    let sent_t_value = original_recipient_balance["transparent_balance"].clone();
    let sent_s_value = original_recipient_balance["sapling_balance"].clone();
    let sent_o_value = original_recipient_balance["orchard_balance"].clone();
    assert_eq!(sent_t_value, 1000u64);
    assert_eq!(sent_s_value, 2000u64);
    assert_eq!(sent_o_value, 3000u64);

    // check that do_rescan works
    original_recipient.do_rescan().await.unwrap();
    check_client_balances!(original_recipient, o: sent_o_value s: sent_s_value t: sent_t_value);

    // Extract viewing keys
    let wc = original_recipient
        .extract_unified_capability()
        .read()
        .await
        .clone();
    use orchard::keys::FullViewingKey as OrchardFvk;
    use zcash_address::unified::Fvk;
    use zcash_primitives::sapling::keys::DiversifiableFullViewingKey as SaplingFvk;
    use zingolib::wallet::keys::extended_transparent::ExtendedPubKey;
    let o_fvk = Fvk::Orchard(OrchardFvk::try_from(&wc).unwrap().to_bytes());
    let s_fvk = Fvk::Sapling(SaplingFvk::try_from(&wc).unwrap().to_bytes());
    let mut t_fvk_bytes = [0u8; 65];
    let t_ext_pk: ExtendedPubKey = (&wc).try_into().unwrap();
    t_fvk_bytes[0..32].copy_from_slice(&t_ext_pk.chain_code[..]);
    t_fvk_bytes[32..65].copy_from_slice(&t_ext_pk.public_key.serialize()[..]);
    let t_fvk = Fvk::P2pkh(t_fvk_bytes);
    let fvks_sets = vec![
        vec![&o_fvk],
        vec![&s_fvk],
        vec![&o_fvk, &s_fvk],
        vec![&o_fvk, &t_fvk],
        vec![&s_fvk, &t_fvk],
        vec![&o_fvk, &s_fvk, &t_fvk],
    ];
    for fvks_set in fvks_sets.iter() {
        log::debug!("testing UFVK containig:");
        log::debug!("    orchard fvk: {}", fvks_set.contains(&&o_fvk));
        log::debug!("    sapling fvk: {}", fvks_set.contains(&&s_fvk));
        log::debug!("    transparent fvk: {}", fvks_set.contains(&&t_fvk));

        use zcash_address::{unified::Encoding, Network::Regtest};
        let ufvk = Ufvk::try_from_items(fvks_set.clone().into_iter().map(|x| x.clone()).collect())
            .unwrap()
            .encode(&Regtest);

        let watch_client =
            LightClient::create_unconnected(&zingo_config, WalletBase::Ufvk(ufvk), 0).unwrap();

        let watch_wc = watch_client
            .extract_unified_capability()
            .read()
            .await
            .clone();

        // assert empty wallet before rescan
        {
            let balance = watch_client.do_balance().await;
            assert_eq!(balance["sapling_balance"], 0);
            assert_eq!(balance["verified_sapling_balance"], 0);
            assert_eq!(balance["unverified_sapling_balance"], 0);
            assert_eq!(balance["orchard_balance"], 0);
            assert_eq!(balance["verified_orchard_balance"], 0);
            assert_eq!(balance["unverified_orchard_balance"], 0);
            assert_eq!(balance["transparent_balance"], 0);
        }

        watch_client.do_rescan().await.unwrap();
        let balance = watch_client.do_balance().await;
        let notes = watch_client.do_list_notes(true).await;

        // Orchard
        if fvks_set.contains(&&o_fvk) {
            assert!(watch_wc.orchard.can_view());
            assert_eq!(balance["orchard_balance"], sent_o_value);
            assert_eq!(balance["verified_orchard_balance"], sent_o_value);
            // assert 1 Orchard note, or 2 notes if a dummy output is included
            let orchard_notes_count = notes["unspent_orchard_notes"].members().count();
            assert!((1..=2).contains(&orchard_notes_count));
        } else {
            assert!(!watch_wc.orchard.can_view());
            assert_eq!(balance["orchard_balance"], 0);
            assert_eq!(balance["verified_orchard_balance"], 0);
            assert_eq!(notes["unspent_orchard_notes"].members().count(), 0);
        }

        // Sapling
        if fvks_set.contains(&&s_fvk) {
            assert!(watch_wc.sapling.can_view());
            assert_eq!(balance["sapling_balance"], sent_s_value);
            assert_eq!(balance["verified_sapling_balance"], sent_s_value);
            assert_eq!(notes["unspent_sapling_notes"].members().count(), 1);
        } else {
            assert!(!watch_wc.sapling.can_view());
            assert_eq!(balance["sapling_balance"], 0);
            assert_eq!(balance["verified_sapling_balance"], 0);
            assert_eq!(notes["unspent_sapling_notes"].members().count(), 0);
        }

        // transparent
        if fvks_set.contains(&&t_fvk) {
            assert!(watch_wc.transparent.can_view());
            assert_eq!(balance["transparent_balance"], sent_t_value);
            assert_eq!(notes["utxos"].members().count(), 1);
        } else {
            assert!(!watch_wc.transparent.can_view());
            assert_eq!(notes["utxos"].members().count(), 0);
        }

        watch_client.do_rescan().await.unwrap();
        assert_eq!(
            watch_client.do_send(vec![(EXT_TADDR, 1000, None)]).await,
            Err("Wallet is in watch-only mode a thus it cannot spend".to_string())
        );
    }
    drop(child_process_handler);
}

#[tokio::test]
async fn zcashd_sapling_commitment_tree() {
    //!  TODO:  Make this test assert something, what is this a test of?
    //!  TODO:  Add doc-comment explaining what constraints this test
    //!  enforces
    let (regtest_manager, child_process_handler, _faucet) = scenarios::faucet().await;
    let trees = regtest_manager
        .get_cli_handle()
        .args(["z_gettreestate", "1"])
        .output()
        .expect("Couldn't get the trees.");
    let trees = json::parse(&String::from_utf8_lossy(&trees.stdout));
    let pretty_trees = json::stringify_pretty(trees.unwrap(), 4);
    println!("{}", pretty_trees);
    drop(child_process_handler);
}

#[tokio::test]
async fn verify_old_wallet_uses_server_height_in_send() {
    //! An earlier version of zingolib used the _wallet's_ 'height' when
    //! constructing transactions.  This worked well enough when the
    //! client completed sync prior to sending, but when we introduced
    //! interrupting send, it made it immediately obvious that this was
    //! the wrong height to use!  The correct height is the
    //! "mempool height" which is the server_height + 1
    let (regtest_manager, child_process_handler, faucet, recipient) =
        scenarios::faucet_recipient().await;
    // Ensure that the client has confirmed spendable funds
    utils::increase_height_and_sync_client(&regtest_manager, &faucet, 5).await;

    // Without sync push server forward 2 blocks
    utils::increase_server_height(&regtest_manager, 2).await;
    let client_wallet_height = faucet.do_wallet_last_scanned_height().await;

    // Verify that wallet is still back at 6.
    assert_eq!(client_wallet_height, 6);

    // Interrupt generating send
    faucet
        .do_send(vec![(
            &get_base_address!(recipient, "unified"),
            10_000,
            Some("Interrupting sync!!".to_string()),
        )])
        .await
        .unwrap();
    drop(child_process_handler);
}
#[tokio::test]
async fn actual_empty_zcashd_sapling_commitment_tree() {
    // Expectations:
    let sprout_commitments_finalroot =
        "59d2cde5e65c1414c32ba54f0fe4bdb3d67618125286e6a191317917c812c6d7";
    let sapling_commitments_finalroot =
        "3e49b5f954aa9d3545bc6c37744661eea48d7c34e3000d82b7f0010c30f4c2fb";
    let orchard_commitments_finalroot =
        "ae2935f1dfd8a24aed7c70df7de3a668eb7a49b1319880dde2bbd9031ae5d82f";
    let finalstates = "000000";
    // Setup
    let (regtest_manager, child_process_handler, _client) = scenarios::basic_no_spendable().await;
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
    //dbg!(std::process::Command::new("grpcurl").args(["-plaintext", "127.0.0.1:9067"]));
    drop(child_process_handler);
}

#[tokio::test]
async fn mine_sapling_to_self() {
    let (regtest_manager, child_process_handler, faucet) = scenarios::faucet().await;
    utils::increase_height_and_sync_client(&regtest_manager, &faucet, 1).await;
    check_client_balances!(faucet, o: 0u64 s: 1_250_000_000u64 t: 0u64);
    drop(child_process_handler);
}

#[tokio::test]
async fn unspent_notes_are_not_saved() {
    let (regtest_manager, child_process_handler, faucet, recipient) =
        scenarios::faucet_recipient().await;
    utils::increase_height_and_sync_client(&regtest_manager, &faucet, 1).await;

    check_client_balances!(faucet, o: 0u64 s: 1_250_000_000u64 t: 0u64);
    faucet
        .do_send(vec![(
            get_base_address!(recipient, "unified").as_str(),
            5_000,
            Some("this note never makes it to the wallet! or chain".to_string()),
        )])
        .await
        .unwrap();

    assert_eq!(
        faucet.do_list_notes(true).await["unspent_orchard_notes"].len(),
        1
    );
    faucet.do_save().await.unwrap();
    // Create a new client using the faucet's wallet

    // Create zingo config
    let mut wallet_location = regtest_manager.zingo_datadir;
    wallet_location.pop();
    wallet_location.push("zingo_client_1");
    let zingo_config = ZingoConfig::create_unconnected(
        zingoconfig::ChainType::Regtest,
        Some(wallet_location.to_string_lossy().to_string()),
    );
    wallet_location.push("zingo-wallet.dat");
    let read_buffer = File::open(wallet_location.clone()).unwrap();

    // Create wallet from faucet zingo-wallet.dat
    let faucet_wallet = zingolib::wallet::LightWallet::read_internal(read_buffer, &zingo_config)
        .await
        .unwrap();

    // Create client based on config and wallet of faucet
    let faucet_copy = LightClient::create_with_wallet(faucet_wallet, zingo_config.clone());
    assert_eq!(
        &faucet_copy.do_seed_phrase().await.unwrap(),
        &faucet.do_seed_phrase().await.unwrap()
    ); // Sanity check identity
    assert_eq!(
        faucet.do_list_notes(true).await["unspent_orchard_notes"].len(),
        1
    );
    assert_eq!(
        faucet_copy.do_list_notes(true).await["unspent_orchard_notes"].len(),
        0
    );
    let mut faucet_transactions = faucet.do_list_transactions(false).await;
    faucet_transactions.pop();
    faucet_transactions.pop();
    let mut faucet_copy_transactions = faucet_copy.do_list_transactions(false).await;
    faucet_copy_transactions.pop();
    assert_eq!(faucet_transactions, faucet_copy_transactions);
    drop(child_process_handler);
}

#[ignore]
#[tokio::test]
async fn load_v26_7d49fdce31() {
    let (_regtest_manager, child_process_handler, client_manager) = scenarios::custom_config(
        "zingocli/tests/data/wallets/v26/202302_release/regtest/zingo-wallet.dat",
    );
    let _zingoconfig = client_manager
        .create_clientconfig(client_manager.zingo_datadir.clone())
        .await;
    //let mut wallet_location = zingo_cli::regtest::get_git_rootdir();
    //wallet_location.push();
    //dbg!(wallet_location);
    //assert_eq!(false, true);
    drop(child_process_handler);
    /*
    let read_buffer = File::open(wallet_location.clone()).unwrap();

    // Create wallet from faucet zingo-wallet.dat
    let faucet_wallet = zingolib::wallet::LightWallet::read_internal(read_buffer, &zingo_config)
        .await
        .unwrap();

    // Create client based on config and wallet of faucet
    let faucet_copy = LightClient::create_with_wallet(faucet_wallet, zingo_config.clone());
    */
}
#[tokio::test]
async fn send_mined_sapling_to_orchard() {
    //! This test shows the 5th confirmation changing the state of balance by
    //! debiting unverified_orchard_balance and crediting verified_orchard_balance.  The debit amount is
    //! consistent with all the notes in the relevant block changing state.
    //! NOTE that the balance doesn't give insight into the distribution across notes.
    let (regtest_manager, child_process_handler, faucet) = scenarios::faucet().await;
    utils::increase_height_and_sync_client(&regtest_manager, &faucet, 1).await;

    let amount_to_send = 5_000;
    faucet
        .do_send(vec![(
            get_base_address!(faucet, "unified").as_str(),
            amount_to_send,
            Some("Scenario test: engage!".to_string()),
        )])
        .await
        .unwrap();

    utils::increase_height_and_sync_client(&regtest_manager, &faucet, 1).await;
    let balance = faucet.do_balance().await;
    // We send change to orchard now, so we should have the full value of the note
    // we spent, minus the transaction fee
    assert_eq!(balance["unverified_orchard_balance"], 0);
    assert_eq!(
        balance["verified_orchard_balance"],
        625_000_000 - u64::from(DEFAULT_FEE)
    );
    drop(child_process_handler);
}
fn extract_value_as_u64(input: &JsonValue) -> u64 {
    let note = &input["value"].as_fixed_point_u64(0).unwrap();
    note.clone()
}

#[tokio::test]
async fn note_selection_order() {
    //! In order to fund a transaction multiple notes may be selected and consumed.
    //! To minimize note selection operations notes are consumed from largest to smallest.
    //! In addition to testing the order in which notes are selected this test:
    //!   * sends to a sapling address
    //!   * sends back to the original sender's UA
    let (regtest_manager, child_process_handler, faucet, recipient) =
        scenarios::faucet_recipient().await;

    utils::increase_height_and_sync_client(&regtest_manager, &faucet, 5).await;

    let client_2_saplingaddress = get_base_address!(recipient, "sapling");
    // Send three transfers in increasing 1000 zat increments
    // These are sent from the coinbase funded client which will
    // subequently receive funding via it's orchard-packed UA.
    faucet
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

    utils::increase_height_and_sync_client(&regtest_manager, &recipient, 5).await;
    // We know that the largest single note that 2 received from 1 was 3000, for 2 to send
    // 3000 back to 1 it will have to collect funds from two notes to pay the full 3000
    // plus the transaction fee.
    recipient
        .do_send(vec![(
            &get_base_address!(faucet, "unified"),
            3000,
            Some("Sending back, should have 2 inputs".to_string()),
        )])
        .await
        .unwrap();
    let client_2_notes = recipient.do_list_notes(false).await;
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

    utils::increase_height_and_sync_client(&regtest_manager, &recipient, 5).await;
    let client_2_post_transaction_notes = recipient.do_list_notes(false).await;
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

    // More explicit than ignoring the unused variable, we only care about this in order to drop it
    drop(child_process_handler);
}

#[tokio::test]
async fn from_t_z_o_tz_to_zo_tzo_to_orchard() {
    //! Test all possible promoting note source combinations
    let (regtest_manager, child_process_handler, mut client_builder) = scenarios::custom_clients();
    let sapling_faucet = client_builder.build_new_faucet(0, false).await;
    let pool_migration_client = client_builder
        .build_newseed_client(HOSPITAL_MUSEUM_SEED.to_string(), 0, false)
        .await;
    let pmc_taddr = get_base_address!(pool_migration_client, "transparent");
    let pmc_sapling = get_base_address!(pool_migration_client, "sapling");
    let pmc_unified = get_base_address!(pool_migration_client, "unified");
    // Ensure that the client has confirmed spendable funds
    utils::increase_height_and_sync_client(&regtest_manager, &sapling_faucet, 3).await;
    // 1 t Test of a send from a taddr only client to its own unified address
    macro_rules! bump_and_check {
        (o: $o:tt s: $s:tt t: $t:tt) => {
            utils::increase_height_and_sync_client(&regtest_manager, &pool_migration_client, 1).await;
            check_client_balances!(pool_migration_client, o:$o s:$s t:$t);
        };
    }

    sapling_faucet
        .do_send(vec![(&pmc_taddr, 5_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 0 s: 0 t: 5_000);

    pool_migration_client
        .do_send(vec![(&pmc_unified, 4_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 4_000 s: 0 t: 0);

    // 2 Test of a send from a sapling only client to its own unified address
    sapling_faucet
        .do_send(vec![(&pmc_sapling, 5_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 4_000 s: 5_000 t: 0);

    pool_migration_client
        .do_send(vec![(&pmc_unified, 4_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 8_000 s: 0 t: 0);

    // 3 Test of an orchard-only client to itself
    pool_migration_client
        .do_send(vec![(&pmc_unified, 7_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 7_000 s: 0 t: 0);

    // 4 tz transparent and sapling to orchard
    pool_migration_client
        .do_send(vec![(&pmc_taddr, 3_000, None), (&pmc_sapling, 3_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 0 s: 3_000 t: 3_000);

    pool_migration_client
        .do_send(vec![(&pmc_unified, 5_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 5_000 s: 0 t: 0);

    // 5 to transparent and orchard to orchard
    pool_migration_client
        .do_send(vec![(&pmc_taddr, 2_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 2_000 s: 0 t: 2_000);

    pool_migration_client
        .do_send(vec![(&pmc_unified, 3_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 3_000 s: 0 t: 0);

    // 6 sapling and orchard to orchard
    sapling_faucet
        .do_send(vec![(&pmc_sapling, 2_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 3_000 s: 2_000 t: 0);

    pool_migration_client
        .do_send(vec![(&pmc_unified, 4_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 4_000 s: 0 t: 0);

    // 7 tzo --> o
    sapling_faucet
        .do_send(vec![(&pmc_taddr, 2_000, None), (&pmc_sapling, 2_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 4_000 s: 2_000 t: 2_000);

    pool_migration_client
        .do_send(vec![(&pmc_unified, 7_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 7_000 s: 0 t: 0);

    // Send from Sapling into empty Orchard pool
    pool_migration_client
        .do_send(vec![(&pmc_sapling, 6_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 0 s: 6_000 t: 0);

    pool_migration_client
        .do_send(vec![(&pmc_unified, 5_000, None)])
        .await
        .unwrap();
    bump_and_check!(o: 5_000 s: 0 t: 0);
    drop(child_process_handler);
}
#[tokio::test]
async fn send_orchard_back_and_forth() {
    // setup
    let (regtest_manager, child_process_handler, faucet, recipient) =
        scenarios::faucet_recipient().await;
    let block_reward = 625_000_000u64;
    let faucet_to_recipient_amount = 10_000u64;
    let recipient_to_faucet_amount = 5_000u64;
    // check start state
    faucet.do_sync(true).await.unwrap();
    let wallet_height = faucet.do_wallet_last_scanned_height().await;
    assert_eq!(wallet_height, 1);
    check_client_balances!(faucet, o: 0 s: block_reward t: 0);

    // post transfer to recipient, and verify
    faucet
        .do_send(vec![(
            &get_base_address!(recipient, "unified"),
            faucet_to_recipient_amount,
            Some("Orcharding".to_string()),
        )])
        .await
        .unwrap();
    let orch_change = block_reward - (faucet_to_recipient_amount + u64::from(DEFAULT_FEE));
    let reward_and_fee = block_reward + u64::from(DEFAULT_FEE);
    utils::increase_height_and_sync_client(&regtest_manager, &recipient, 1).await;
    faucet.do_sync(true).await.unwrap();
    check_client_balances!(recipient, o: faucet_to_recipient_amount s: 0 t: 0);
    check_client_balances!(faucet, o: orch_change s: reward_and_fee t: 0);

    // post half back to faucet, and verify
    recipient
        .do_send(vec![(
            &get_base_address!(faucet, "unified"),
            recipient_to_faucet_amount,
            Some("Sending back".to_string()),
        )])
        .await
        .unwrap();
    utils::increase_height_and_sync_client(&regtest_manager, &faucet, 1).await;
    recipient.do_sync(true).await.unwrap();

    let recipient_final_orch =
        faucet_to_recipient_amount - (u64::from(DEFAULT_FEE) + recipient_to_faucet_amount);
    let faucet_final_orch = orch_change + recipient_to_faucet_amount;
    let faucet_final_block = 2 * block_reward + u64::from(DEFAULT_FEE) * 2;
    check_client_balances!(
        faucet,
        o: faucet_final_orch s: faucet_final_block t: 0
    );
    check_client_balances!(recipient, o: recipient_final_orch s: 0 t: 0);
    drop(child_process_handler);
}

#[tokio::test]
async fn diversified_addresses_receive_funds_in_best_pool() {
    let (regtest_manager, child_process_handler, faucet, recipient) =
        scenarios::faucet_recipient().await;
    for code in ["o", "zo", "z"] {
        recipient.do_new_address(code).await.unwrap();
    }
    let addresses = recipient.do_addresses().await;
    let address_5000_nonememo_tuples = addresses
        .members()
        .map(|ua| (ua["address"].as_str().unwrap(), 5_000, None))
        .collect::<Vec<(&str, u64, Option<String>)>>();
    faucet.do_send(address_5000_nonememo_tuples).await.unwrap();
    utils::increase_height_and_sync_client(&regtest_manager, &recipient, 1).await;
    let balance_b = recipient.do_balance().await;
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
}

#[tokio::test]
async fn rescan_still_have_outgoing_metadata() {
    let (regtest_manager, child_process_handler, faucet, recipient) =
        scenarios::faucet_recipient().await;
    faucet
        .do_send(vec![(
            get_base_address!(recipient, "sapling").as_str(),
            1_000,
            Some("foo".to_string()),
        )])
        .await
        .unwrap();
    utils::increase_height_and_sync_client(&regtest_manager, &faucet, 1).await;
    let transactions = faucet.do_list_transactions(false).await;
    faucet.do_rescan().await.unwrap();
    let post_rescan_transactions = faucet.do_list_transactions(false).await;
    assert_eq!(transactions, post_rescan_transactions);

    drop(child_process_handler);
}

#[tokio::test]
async fn rescan_still_have_outgoing_metadata_with_sends_to_self() {
    let (regtest_manager, child_process_handler, faucet) = scenarios::faucet().await;
    let sapling_addr = get_base_address!(faucet, "sapling");
    for memo in [None, Some("foo")] {
        faucet
            .do_send(vec![(
                sapling_addr.as_str(),
                {
                    let balance = faucet.do_balance().await;
                    balance["spendable_sapling_balance"].as_u64().unwrap()
                        + balance["spendable_orchard_balance"].as_u64().unwrap()
                } - 1_000,
                memo.map(ToString::to_string),
            )])
            .await
            .unwrap();
        utils::increase_height_and_sync_client(&regtest_manager, &faucet, 1).await;
    }
    let transactions = faucet.do_list_transactions(false).await;
    let notes = faucet.do_list_notes(true).await;
    faucet.do_rescan().await.unwrap();
    let post_rescan_transactions = faucet.do_list_transactions(false).await;
    let post_rescan_notes = faucet.do_list_notes(true).await;
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
}

/// An arbitrary number of diversified addresses may be generated
/// from a seed.  If the wallet is subsequently lost-or-destroyed
/// wallet-regeneration-from-seed (sprouting) doesn't regenerate
/// the previous diversifier list. <-- But the spend capability
/// is capable of recovering the diversified _receiver_.
#[tokio::test]
async fn handling_of_nonregenerated_diversified_addresses_after_seed_restore() {
    let (regtest_manager, child_process_handler, mut client_builder) = scenarios::custom_clients();
    let faucet = client_builder.build_new_faucet(0, false).await;
    faucet.do_sync(false).await.unwrap();
    let seed_phrase_of_recipient1 = zcash_primitives::zip339::Mnemonic::from_entropy([1; 32])
        .unwrap()
        .to_string();
    let recipient1 = client_builder
        .build_newseed_client(seed_phrase_of_recipient1, 0, false)
        .await;
    let mut expected_unspent_sapling_notes = json::object! {

            "created_in_block" =>  2,
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
    let seed_of_recipient = {
        assert_eq!(
            &get_base_address!(recipient1, "unified"),
            &original_recipient_address
        );
        let recipient_addr = recipient1.do_new_address("tz").await.unwrap();
        faucet
            .do_send(vec![(
                recipient_addr[0].as_str().unwrap(),
                5_000,
                Some("foo".to_string()),
            )])
            .await
            .unwrap();
        utils::increase_height_and_sync_client(&regtest_manager, &faucet, 1).await;
        recipient1.do_sync(true).await.unwrap();
        let notes = recipient1.do_list_notes(true).await;
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
        recipient1.do_seed_phrase().await.unwrap()
    };
    drop(recipient1); // Discard original to ensure subsequent data is fresh.
    let mut expected_unspent_sapling_notes_after_restore_from_seed =
        expected_unspent_sapling_notes.clone();
    expected_unspent_sapling_notes_after_restore_from_seed["address"] = JsonValue::String(
        "Diversifier not in wallet. Perhaps you restored from seed and didn't restore addresses"
            .to_string(),
    );
    let recipient_restored = client_builder
        .build_newseed_client(
            seed_of_recipient["seed"].as_str().unwrap().to_string(),
            0,
            true,
        )
        .await;
    let seed_of_recipient_restored = {
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
        recipient_restored
            .do_send(vec![(&get_base_address!(faucet, "unified"), 4_000, None)])
            .await
            .unwrap();
        let sender_balance = faucet.do_balance().await;
        utils::increase_height_and_sync_client(&regtest_manager, &faucet, 1).await;

        //Ensure that recipient_restored was still able to spend the note, despite not having the
        //diversified address associated with it
        assert_eq!(
            faucet.do_balance().await["spendable_orchard_balance"],
            sender_balance["spendable_orchard_balance"]
                .as_u64()
                .unwrap()
                + 4_000
        );
        recipient_restored.do_seed_phrase().await.unwrap()
    };
    assert_eq!(seed_of_recipient, seed_of_recipient_restored);
    drop(child_process_handler);
}

#[tokio::test]
async fn ensure_taddrs_from_old_seeds_work() {
    let (_regtest_manager, child_process_handler, mut client_builder) = scenarios::custom_clients();
    // The first taddr generated on commit 9e71a14eb424631372fd08503b1bd83ea763c7fb
    let transparent_address = "tmFLszfkjgim4zoUMAXpuohnFBAKy99rr2i";

    let client_b = client_builder
        .build_newseed_client(HOSPITAL_MUSEUM_SEED.to_string(), 0, false)
        .await;

    assert_eq!(
        get_base_address!(client_b, "transparent"),
        transparent_address
    );
    drop(child_process_handler);
}

#[tokio::test]
async fn t_incoming_t_outgoing() {
    let (regtest_manager, child_process_handler, faucet, recipient) =
        scenarios::faucet_recipient().await;

    // 2. Get an incoming transaction to a t address
    let taddr = get_base_address!(recipient, "transparent");
    let value = 100_000;

    faucet
        .do_send(vec![(taddr.as_str(), value, None)])
        .await
        .unwrap();

    utils::increase_height_and_sync_client(&regtest_manager, &recipient, 1).await;
    recipient.do_sync(true).await.unwrap();

    // 3. Test the list
    let list = recipient.do_list_transactions(false).await;
    assert_eq!(list[0]["block_height"].as_u64().unwrap(), 2);
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
    assert_eq!(list[1]["block_height"].as_u64().unwrap(), 3);
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
        2
    );
    assert_eq!(
        notes["spent_utxos"][0]["spent_at_height"].as_u64().unwrap(),
        3
    );
    assert_eq!(notes["spent_utxos"][0]["spent"], sent_transaction_id);

    // Change shielded note
    assert_eq!(
        notes["unspent_orchard_notes"][0]["created_in_block"]
            .as_u64()
            .unwrap(),
        3
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

    assert_eq!(list[1]["block_height"].as_u64().unwrap(), 3);
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
    assert_eq!(list[1]["block_height"].as_u64().unwrap(), 3);
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
        3
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
    drop(child_process_handler);
}

#[tokio::test]
async fn send_to_ua_saves_full_ua_in_wallet() {
    let (regtest_manager, child_process_handler, faucet, recipient) =
        scenarios::faucet_recipient().await;
    //utils::increase_height_and_sync_client(&regtest_manager, &faucet, 5).await;
    let recipient_unified_address = get_base_address!(recipient, "unified");
    let sent_value = 50_000;
    faucet
        .do_send(vec![(recipient_unified_address.as_str(), sent_value, None)])
        .await
        .unwrap();
    utils::increase_height_and_sync_client(&regtest_manager, &faucet, 1).await;
    let list = faucet.do_list_transactions(false).await;
    assert!(list.members().any(|transaction| {
        transaction.entries().any(|(key, value)| {
            if key == "outgoing_metadata" {
                value[0]["address"] == recipient_unified_address
            } else {
                false
            }
        })
    }));
    faucet.do_rescan().await.unwrap();
    let new_list = faucet.do_list_transactions(false).await;
    assert!(new_list.members().any(|transaction| {
        transaction.entries().any(|(key, value)| {
            if key == "outgoing_metadata" {
                value[0]["address"] == recipient_unified_address
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
    drop(child_process_handler);
}

#[tokio::test]
async fn self_send_to_t_displays_as_one_transaction() {
    let (regtest_manager, child_process_handler, faucet, recipient) =
        scenarios::faucet_recipient().await;
    let recipient_unified_address = get_base_address!(recipient, "unified");
    let sent_value = 50_000;
    faucet
        .do_send(vec![(recipient_unified_address.as_str(), sent_value, None)])
        .await
        .unwrap();
    utils::increase_height_and_sync_client(&regtest_manager, &recipient, 1).await;
    let recipient_taddr = get_base_address!(recipient, "transparent");
    let recipient_zaddr = get_base_address!(recipient, "sapling");
    let sent_to_taddr_value = 5_000;
    let sent_to_zaddr_value = 11_000;
    let sent_to_self_orchard_value = 1_000;
    recipient
        .do_send(vec![(recipient_taddr.as_str(), sent_to_taddr_value, None)])
        .await
        .unwrap();
    utils::increase_height_and_sync_client(&regtest_manager, &recipient, 1).await;
    recipient
        .do_send(vec![
            (recipient_taddr.as_str(), sent_to_taddr_value, None),
            (
                recipient_zaddr.as_str(),
                sent_to_zaddr_value,
                Some("foo".to_string()),
            ),
            (
                recipient_unified_address.as_str(),
                sent_to_self_orchard_value,
                Some("bar".to_string()),
            ),
        ])
        .await
        .unwrap();
    faucet.do_sync(false).await.unwrap();
    faucet
        .do_send(vec![
            (recipient_taddr.as_str(), sent_to_taddr_value, None),
            (
                recipient_zaddr.as_str(),
                sent_to_zaddr_value,
                Some("foo2".to_string()),
            ),
            (
                recipient_unified_address.as_str(),
                sent_to_self_orchard_value,
                Some("bar2".to_string()),
            ),
        ])
        .await
        .unwrap();
    utils::increase_height_and_sync_client(&regtest_manager, &recipient, 1).await;
    println!(
        "{}",
        json::stringify_pretty(recipient.do_list_transactions(false).await, 4)
    );
    let transactions = recipient.do_list_transactions(false).await;
    let mut txids = transactions
        .members()
        .map(|transaction| transaction["txid"].as_str());
    assert!(itertools::Itertools::all_unique(&mut txids));
    drop(child_process_handler);
}

// Burn-to regtest address generated by `zcash-cli getnewaddress`
const EXT_TADDR: &str = "tmJTBtMwPU96XteSiP89xDz1WARNgRddEHq";
#[cfg(feature = "cross_version")]
#[tokio::test]
async fn cross_compat() {
    let (_regtest_manager, child_process_handler, current_client, fixed_address_client) =
        scenarios::current_and_fixed_clients().await;

    let fixed_taddr_seed = fixed_address_client.do_seed_phrase().await.unwrap();
    let current_seed = current_client.do_seed_phrase().await.unwrap();
    assert_eq!(fixed_taddr_seed["seed"], current_seed["seed"]);
    assert_eq!(
        get_base_address!(fixed_address_client, "unified"),
        get_base_address!(current_client, "unified")
    );
    drop(child_process_handler);
}

#[tokio::test]
async fn sapling_to_sapling_scan_together() {
    // Create an incoming transaction, and then send that transaction, and scan everything together, to make sure it works.
    // (For this test, the Sapling Domain is assumed in all cases.)
    // Sender Setup:
    // 1. create a spend key: SpendK_S
    // 2. derive a Shielded Payment Address from SpendK_S: SPA_KS
    // 3. construct a Block Reward Transaction where SPA_KS receives a block reward: BRT
    // 4. publish BRT
    // 5. optionally mine a block including BRT <-- There are two separate tests to run
    // 6. optionally mine sufficient subsequent blocks to "validate" BRT
    // Recipient Setup:
    // 1. create a spend key: "SpendK_R"
    // 2. from SpendK_R derive a Shielded Payment Address: SPA_R
    // Test Procedure:
    // 1. construct a transaction "spending" from a SpendK_S output to SPA_R
    // 2. publish the transaction to the mempool
    // 3. mine a block
    // Constraints:
    // 1. SpendK_S controls start - spend funds
    // 2. SpendK_R controls 0 + spend funds
    let (regtest_manager, child_process_handler, faucet, recipient) =
        scenarios::faucet_recipient().await;

    // Give the faucet a block reward
    utils::increase_height_and_sync_client(&regtest_manager, &faucet, 1).await;
    let value = 100_000;

    // Send some sapling value to the recipient
    let txid = utils::send_value_between_clients_and_sync(
        &regtest_manager,
        &faucet,
        &recipient,
        value,
        "sapling",
    )
    .await;

    let spent_value = 250;

    // Construct transaction to wallet-external recipient-address.
    let exit_zaddr = get_base_address!(faucet, "sapling");
    let spent_txid = recipient
        .do_send(vec![(&exit_zaddr, spent_value, None)])
        .await
        .unwrap();

    utils::increase_height_and_sync_client(&regtest_manager, &recipient, 1).await;
    // 5. Check the transaction list to make sure we got all transactions
    let list = recipient.do_list_transactions(false).await;

    assert_eq!(list[0]["block_height"].as_u64().unwrap(), 3);
    assert_eq!(list[0]["txid"], txid.to_string());
    assert_eq!(list[0]["amount"].as_i64().unwrap(), (value as i64));

    assert_eq!(list[1]["block_height"].as_u64().unwrap(), 4);
    assert_eq!(list[1]["txid"], spent_txid.to_string());
    assert_eq!(
        list[1]["amount"].as_i64().unwrap(),
        -((spent_value + u64::from(DEFAULT_FEE)) as i64)
    );
    assert_eq!(list[1]["outgoing_metadata"][0]["address"], exit_zaddr);
    assert_eq!(
        list[1]["outgoing_metadata"][0]["value"].as_u64().unwrap(),
        spent_value
    );

    drop(child_process_handler);
}

#[tokio::test]
async fn mixed_transaction() {
    let zvalue = 100_000;
    let (regtest_manager, child_process_handler, faucet, recipient, _txid) =
        scenarios::faucet_prefunded_orchard_recipient(zvalue).await;

    // 3. Send an incoming t-address transaction
    let tvalue = 200_000;
    utils::send_value_between_clients_and_sync(
        &regtest_manager,
        &faucet,
        &recipient,
        tvalue,
        "transparent",
    )
    .await;

    let (faucet_sapling, faucet_transparent) = (
        get_base_address!(faucet, "sapling"),
        get_base_address!(faucet, "transparent"),
    );
    // 4. Send a transaction to both external t-addr and external z addr and mine it
    let sent_zvalue = 80_000;
    let sent_tvalue = 140_000;
    let sent_zmemo = "Ext z".to_string();
    let tos = vec![
        (
            faucet_sapling.as_str(),
            sent_zvalue,
            Some(sent_zmemo.clone()),
        ),
        (faucet_transparent.as_str(), sent_tvalue, None),
    ];
    recipient.do_send(tos).await.unwrap();

    utils::increase_height_and_sync_client(&regtest_manager, &recipient, 1).await;

    let notes = recipient.do_list_notes(true).await;
    let list = recipient.do_list_transactions(false).await;

    // 5. Check everything
    assert_eq!(notes["unspent_orchard_notes"].len(), 1);
    assert_eq!(
        notes["unspent_orchard_notes"][0]["created_in_block"]
            .as_u64()
            .unwrap(),
        5
    );
    assert_eq!(
        notes["unspent_orchard_notes"][0]["is_change"]
            .as_bool()
            .unwrap(),
        true
    );
    assert_eq!(
        notes["unspent_orchard_notes"][0]["value"].as_u64().unwrap(),
        tvalue + zvalue - sent_tvalue - sent_zvalue - u64::from(DEFAULT_FEE)
    );

    assert_eq!(notes["spent_orchard_notes"].len(), 1);
    assert_eq!(
        notes["spent_orchard_notes"][0]["spent"],
        notes["unspent_orchard_notes"][0]["created_in_txid"]
    );

    assert_eq!(notes["pending_sapling_notes"].len(), 0);
    assert_eq!(notes["pending_orchard_notes"].len(), 0);
    assert_eq!(notes["utxos"].len(), 0);
    assert_eq!(notes["pending_utxos"].len(), 0);

    assert_eq!(notes["spent_utxos"].len(), 1);
    assert_eq!(
        notes["spent_utxos"][0]["spent"],
        notes["unspent_orchard_notes"][0]["created_in_txid"]
    );

    assert_eq!(list.len(), 3);
    assert_eq!(list[2]["block_height"].as_u64().unwrap(), 5);
    assert_eq!(
        list[2]["amount"].as_i64().unwrap(),
        0 - (sent_tvalue + sent_zvalue + u64::from(DEFAULT_FEE)) as i64
    );
    assert_eq!(
        list[2]["txid"],
        notes["unspent_orchard_notes"][0]["created_in_txid"]
    );
    assert_eq!(
        list[2]["outgoing_metadata"]
            .members()
            .find(|j| j["address"].to_string() == faucet_sapling
                && j["value"].as_u64().unwrap() == sent_zvalue)
            .unwrap()["memo"]
            .to_string(),
        sent_zmemo
    );
    assert_eq!(
        list[2]["outgoing_metadata"]
            .members()
            .find(|j| j["address"].to_string() == faucet_transparent)
            .unwrap()["value"]
            .as_u64()
            .unwrap(),
        sent_tvalue
    );

    drop(child_process_handler);
}

#[tokio::test]
async fn load_wallet_from_v26_dat_file() {
    // We test that the LightWallet can be read from v26 .dat file
    // Changes in version 27:
    //   - The wallet does not have to have a mnemonic.
    //     Absence of mnemonic is represented by an empty byte vector in v27.
    //     v26 serialized wallet is always loaded with `Some(mnemonic)`.
    //   - The wallet capabilities can be restricted from spending to view-only or none.
    //     We introduce `Capability` type represent different capability types in v27.
    //     v26 serialized wallet is always loaded with `Capability::Spend(sk)`.

    // A testnet wallet initiated with
    // --seed "chimney better bulb horror rebuild whisper improve intact letter giraffe brave rib appear bulk aim burst snap salt hill sad merge tennis phrase raise"
    // --birthday 0
    // --nosync
    // with 3 addresses containig all receivers.
    let data = include_bytes!("zingo-wallet-v26.dat");

    let config = zingoconfig::ZingoConfig::create_unconnected(ChainType::Testnet, None);
    let wallet = LightWallet::read_internal(&data[..], &config)
        .await
        .map_err(|e| format!("Cannot deserialize LightWallet version 26 file: {}", e))
        .unwrap();

    let expected_mnemonic = Mnemonic::from_phrase(TEST_SEED.to_string()).unwrap();
    assert_eq!(wallet.mnemonic(), Some(&expected_mnemonic));

    let expected_wc = WalletCapability::new_from_phrase(&config, &expected_mnemonic, 0).unwrap();
    let wc = wallet.wallet_capability().read().await.clone();

    // We don't want the WalletCapability to impl. `Eq` (because it stores secret keys)
    // so we have to compare each component instead

    // Compare Orchard
    let Capability::Spend(orchard_sk) = &wc.orchard else {
        panic!("Expected Orchard Spending Key");
    };
    assert_eq!(
        orchard_sk.to_bytes(),
        orchard::keys::SpendingKey::try_from(&expected_wc)
            .unwrap()
            .to_bytes()
    );

    // Compare Sapling
    let Capability::Spend(sapling_sk) = &wc.sapling else {
        panic!("Expected Sapling Spending Key");
    };
    assert_eq!(
        sapling_sk,
        &zcash_primitives::zip32::ExtendedSpendingKey::try_from(&expected_wc).unwrap()
    );

    // Compare transparent
    let Capability::Spend(transparent_sk) = &wc.transparent else {
        panic!("Expected transparent extended private key");
    };
    assert_eq!(
        transparent_sk,
        &ExtendedPrivKey::try_from(&expected_wc).unwrap()
    );

    assert_eq!(wc.addresses().len(), 3);
    for addr in wc.addresses() {
        assert!(addr.orchard().is_some());
        assert!(addr.sapling().is_some());
        assert!(addr.transparent().is_some());
    }
}

pub const TEST_SEED: &str = "chimney better bulb horror rebuild whisper improve intact letter giraffe brave rib appear bulk aim burst snap salt hill sad merge tennis phrase raise";

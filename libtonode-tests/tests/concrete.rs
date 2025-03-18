#![forbid(unsafe_code)]

use json::JsonValue;
use testvectors::{block_rewards, seeds::HOSPITAL_MUSEUM_SEED, BASE_HEIGHT};
use zcash_address::unified::Fvk;
use zcash_primitives::transaction::components::amount::NonNegativeAmount;
use zcash_primitives::transaction::fees::zip317::MINIMUM_FEE;
use zingolib::config::RegtestNetwork;
use zingolib::lightclient::PoolBalances;
use zingolib::testutils::lightclient::from_inputs;
use zingolib::testutils::{increase_height_and_wait_for_client, scenarios};
use zingolib::utils::conversion::address_from_str;
use zingolib::wallet::keys::unified::UnifiedKeyStore;
use zingolib::wallet::propose::ProposeSendError;
use zingolib::wallet::summary::{CoinSummary, NoteSummary};
use zingolib::{check_client_balances, get_base_address_macro};

fn check_expected_balance_with_fvks(
    fvks: &Vec<&Fvk>,
    balance: PoolBalances,
    o_expect: u64,
    s_expect: u64,
    t_expect: u64,
) {
    for fvk in fvks {
        match fvk {
            Fvk::Sapling(_) => {
                assert_eq!(balance.sapling_balance.unwrap(), s_expect);
                assert_eq!(balance.verified_sapling_balance.unwrap(), s_expect);
                assert_eq!(balance.unverified_sapling_balance.unwrap(), s_expect);
            }
            Fvk::Orchard(_) => {
                assert_eq!(balance.orchard_balance.unwrap(), o_expect);
                assert_eq!(balance.verified_orchard_balance.unwrap(), o_expect);
                assert_eq!(balance.unverified_orchard_balance.unwrap(), o_expect);
            }
            Fvk::P2pkh(_) => {
                assert_eq!(balance.transparent_balance.unwrap(), t_expect);
            }
            _ => panic!(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check_view_capability_bounds(
    balance: &PoolBalances,
    unified_key_store: &UnifiedKeyStore,
    fvks: &[&Fvk],
    orchard_fvk: &Fvk,
    sapling_fvk: &Fvk,
    transparent_fvk: &Fvk,
    sent_o_value: Option<u64>,
    sent_s_value: Option<u64>,
    sent_t_value: Option<u64>,
    orchard_notes: &[NoteSummary],
    sapling_notes: &[NoteSummary],
    transparent_coins: &[CoinSummary],
) {
    let UnifiedKeyStore::View(ufvk) = unified_key_store else {
        panic!("should be viewing key!")
    };
    //Orchard
    if !fvks.contains(&orchard_fvk) {
        assert!(ufvk.orchard().is_none());
        assert_eq!(balance.orchard_balance, None);
        assert_eq!(balance.verified_orchard_balance, None);
        assert_eq!(balance.unverified_orchard_balance, None);
        assert_eq!(orchard_notes.len(), 0);
    } else {
        assert!(ufvk.orchard().is_some());
        assert_eq!(balance.orchard_balance, sent_o_value);
        assert_eq!(balance.verified_orchard_balance, sent_o_value);
        assert_eq!(balance.unverified_orchard_balance, Some(0));
        // assert 1 Orchard note, or 2 notes if a dummy output is included
        let orchard_notes_count = orchard_notes
            .iter()
            .filter(|note| note.spend_status.is_unspent())
            .count();
        assert!((1..=2).contains(&orchard_notes_count));
    }
    //Sapling
    if !fvks.contains(&sapling_fvk) {
        assert!(ufvk.sapling().is_none());
        assert_eq!(balance.sapling_balance, None);
        assert_eq!(balance.verified_sapling_balance, None);
        assert_eq!(balance.unverified_sapling_balance, None);
        assert_eq!(sapling_notes.len(), 0);
    } else {
        assert!(ufvk.sapling().is_some());
        assert_eq!(balance.sapling_balance, sent_s_value);
        assert_eq!(balance.verified_sapling_balance, sent_s_value);
        assert_eq!(balance.unverified_sapling_balance, Some(0));
        assert_eq!(
            sapling_notes
                .iter()
                .filter(|note| note.spend_status.is_unspent())
                .count(),
            1
        );
    }
    if !fvks.contains(&transparent_fvk) {
        assert!(ufvk.transparent().is_none());
        assert_eq!(balance.transparent_balance, None);
        assert_eq!(transparent_coins.len(), 0);
    } else {
        assert!(ufvk.transparent().is_some());
        assert_eq!(balance.transparent_balance, sent_t_value);
        assert_eq!(transparent_coins.len(), 1);
    }
}

mod fast {
    use std::str::FromStr as _;

    use bip0039::Mnemonic;
    use pepper_sync::wallet::{OutputInterface, TransparentCoin};
    use zcash_address::ZcashAddress;
    use zcash_client_backend::{
        zip321::{Payment, TransactionRequest},
        PoolType, ShieldedProtocol,
    };
    use zcash_primitives::{
        consensus::BlockHeight, memo::Memo, transaction::components::amount::NonNegativeAmount,
    };
    use zingo_status::confirmation_status::ConfirmationStatus;
    use zingolib::{
        config::ZENNIES_FOR_ZINGO_REGTEST_ADDRESS,
        lightclient::describe::UAReceivers,
        testutils::{
            chain_generics::{conduct_chain::ConductChain, libtonode::LibtonodeEnvironment},
            lightclient::{from_inputs, get_base_address},
        },
        wallet::{
            data::summaries::{
                SelfSendValueTransfer, SentValueTransfer, TransactionSummaryInterface as _,
                ValueTransferKind,
            },
            keys::unified::ReceiverSelection,
        },
    };

    use super::*;

    // FIXME: zingo2, large test to re-integrate
    // #[tokio::test]
    // async fn mempool_clearing_and_full_batch_syncs_correct_trees() {
    //     async fn do_maybe_recent_txid(lc: &LightClient) -> JsonValue {
    //         json::object! {
    //             "last_txid" => lc.wallet.transactions().read().await.get_some_txid_from_highest_wallet_block().map(|t| t.to_string())
    //         }
    //     }
    //     let value = 100_000;
    //     let regtest_network = RegtestNetwork::all_upgrades_active();
    //     let (regtest_manager, _cph, faucet, recipient, orig_transaction_id, _, _) =
    //         scenarios::faucet_funded_recipient(
    //             Some(value),
    //             None,
    //             None,
    //             PoolType::Shielded(ShieldedProtocol::Sapling),
    //             regtest_network,
    //             true,
    //         )
    //         .await;
    //     let orig_transaction_id = orig_transaction_id.unwrap();
    //     assert_eq!(
    //         do_maybe_recent_txid(&recipient).await["last_txid"],
    //         orig_transaction_id
    //     );
    //     // Put some transactions unrelated to the recipient (faucet->faucet) on-chain, to get some clutter
    //     for _ in 0..5 {
    //         zingolib::testutils::send_value_between_clients_and_sync(
    //             &regtest_manager,
    //             &faucet,
    //             &faucet,
    //             5_000,
    //             "unified",
    //         )
    //         .await
    //         .unwrap();
    //     }

    //     let sent_to_self = 10;
    //     // Send recipient->recipient, to make tree equality check at the end simpler
    //     zingolib::testutils::send_value_between_clients_and_sync(
    //         &regtest_manager,
    //         &recipient,
    //         &recipient,
    //         sent_to_self,
    //         "unified",
    //     )
    //     .await
    //     .unwrap();
    //     let fees = zingolib::testutils::lightclient::get_fees_paid_by_client(&recipient).await;
    //     assert_eq!(value - fees, 90_000);
    //     let balance_minus_step_one_fees = value - fees;

    //     // 3a. stash zcashd state
    //     log::debug!(
    //         "old zcashd chain info {}",
    //         std::str::from_utf8(
    //             &regtest_manager
    //                 .get_cli_handle()
    //                 .arg("getblockchaininfo")
    //                 .output()
    //                 .unwrap()
    //                 .stdout
    //         )
    //         .unwrap()
    //     );

    //     // Turn zcashd off and on again, to write down the blocks
    //     drop(_cph); // turn off zcashd and lightwalletd
    //     let _cph = regtest_manager.launch(false).unwrap();
    //     log::debug!(
    //         "new zcashd chain info {}",
    //         std::str::from_utf8(
    //             &regtest_manager
    //                 .get_cli_handle()
    //                 .arg("getblockchaininfo")
    //                 .output()
    //                 .unwrap()
    //                 .stdout
    //         )
    //         .unwrap()
    //     );

    //     let zcd_datadir = &regtest_manager.zcashd_data_dir;
    //     let zcashd_parent = Path::new(zcd_datadir).parent().unwrap();
    //     let original_zcashd_directory = zcashd_parent.join("original_zcashd");

    //     log::debug!(
    //         "The original zcashd directory is at: {}",
    //         &original_zcashd_directory.to_string_lossy().to_string()
    //     );

    //     let source = &zcd_datadir.to_string_lossy().to_string();
    //     let dest = &original_zcashd_directory.to_string_lossy().to_string();
    //     std::process::Command::new("cp")
    //         .arg("-rf")
    //         .arg(source)
    //         .arg(dest)
    //         .output()
    //         .expect("directory copy failed");

    //     // 3. Send z-to-z transaction to external z address with a memo
    //     let sent_value = 2000;
    //     let outgoing_memo = "Outgoing Memo";

    //     let sent_transaction_id = from_inputs::quick_send(
    //         &recipient,
    //         vec![(
    //             &get_base_address_macro!(faucet, "sapling"),
    //             sent_value,
    //             Some(outgoing_memo),
    //         )],
    //     )
    //     .await
    //     .unwrap()
    //     .first()
    //     .to_string();

    //     let second_transaction_fee;
    //     {
    //         let tmds = recipient
    //             .wallet
    //             .transaction_context
    //             .transaction_metadata_set
    //             .read()
    //             .await;
    //         let record = tmds
    //             .transaction_records_by_id
    //             .get(
    //                 &crate::utils::conversion::txid_from_hex_encoded_str(&sent_transaction_id)
    //                     .unwrap(),
    //             )
    //             .unwrap();
    //         second_transaction_fee = tmds
    //             .transaction_records_by_id
    //             .calculate_transaction_fee(record)
    //             .unwrap();
    //         // Sync recipient
    //     } // drop transaction_record references and tmds read lock
    //     recipient.do_sync(false).await.unwrap();

    //     // 4b write down state before clearing the mempool
    //     let notes_before = recipient.do_list_notes(true).await;
    //     let transactions_before = recipient.do_list_transactions().await;

    //     // Sync recipient again. We assert this should be a no-op, as we just synced
    //     recipient.do_sync(false).await.unwrap();
    //     let post_sync_notes_before = recipient.do_list_notes(true).await;
    //     let post_sync_transactions_before = recipient.do_list_transactions().await;
    //     assert_eq!(post_sync_notes_before, notes_before);
    //     assert_eq!(post_sync_transactions_before, transactions_before);

    //     drop(_cph); // Turn off zcashd and lightwalletd

    //     // 5. check that the sent transaction is correctly marked in the client
    //     let transactions = recipient.do_list_transactions().await;
    //     let mempool_only_tx = transactions
    //         .members()
    //         .find(|tx| tx["txid"] == sent_transaction_id)
    //         .unwrap()
    //         .clone();
    //     dbg!(&mempool_only_tx["txid"]);
    //     assert_eq!(
    //         mempool_only_tx["outgoing_metadata"][0]["memo"],
    //         "Outgoing Memo"
    //     );
    //     assert_eq!(mempool_only_tx["txid"], sent_transaction_id);

    //     // 6. note that the client correctly considers the note pending
    //     assert_eq!(mempool_only_tx["pending"], true);

    //     std::process::Command::new("rm")
    //         .arg("-rf")
    //         .arg(source)
    //         .output()
    //         .expect("recursive rm failed");
    //     std::process::Command::new("cp")
    //         .arg("--recursive")
    //         .arg("--remove-destination")
    //         .arg(dest)
    //         .arg(source)
    //         .output()
    //         .expect("directory copy failed");
    //     assert_eq!(
    //         source,
    //         &regtest_manager
    //             .zcashd_data_dir
    //             .to_string_lossy()
    //             .to_string()
    //     );
    //     let _cph = regtest_manager.launch(false).unwrap();
    //     let notes_after = recipient.do_list_notes(true).await;
    //     let transactions_after = recipient.do_list_transactions().await;

    //     assert_eq!(notes_before.pretty(2), notes_after.pretty(2));
    //     assert_eq!(transactions_before.pretty(2), transactions_after.pretty(2));

    //     // 6. Mine 10 blocks, the pending transaction should still be there.
    //     zingolib::testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
    //         .await
    //         .unwrap();
    //     assert_eq!(recipient.wallet.last_synced_height().await, 12);

    //     let notes = recipient.do_list_notes(true).await;

    //     let transactions = recipient.do_list_transactions().await;

    //     // There are 2 unspent notes, the pending transaction, and the final receipt
    //     //println!("{}", json::stringify_pretty(notes.clone(), 4));
    //     //println!("{}", json::stringify_pretty(transactions.clone(), 4));
    //     // Two unspent notes: one change, pending, one from faucet, confirmed
    //     assert_eq!(notes["unspent_orchard_notes"].len(), 2);
    //     assert_eq!(notes["unspent_sapling_notes"].len(), 0);
    //     let note = notes["unspent_orchard_notes"][1].clone();
    //     assert_eq!(note["created_in_txid"], sent_transaction_id);
    //     assert_eq!(
    //         note["value"].as_u64().unwrap(),
    //         balance_minus_step_one_fees - sent_value - second_transaction_fee - sent_to_self
    //     );
    //     assert!(note["pending"].as_bool().unwrap());
    //     assert_eq!(transactions.len(), 3);

    //     // 7. Mine 3 blocks, so the 2 block pending_window is passed
    //     zingolib::testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 3)
    //         .await
    //         .unwrap();
    //     assert_eq!(recipient.wallet.last_synced_height().await, 15);

    //     let notes = recipient.do_list_notes(true).await;
    //     let transactions = recipient.do_list_transactions().await;

    //     // There are now three notes, the original (confirmed and spent) note, the send to self note, and its change.
    //     assert_eq!(notes["unspent_orchard_notes"].len(), 2);
    //     assert_eq!(
    //         notes["spent_orchard_notes"][0]["created_in_txid"],
    //         orig_transaction_id
    //     );
    //     assert!(!notes["unspent_orchard_notes"][0]["pending"]
    //         .as_bool()
    //         .unwrap());
    //     assert_eq!(notes["pending_orchard_notes"].len(), 0);
    //     assert_eq!(transactions.len(), 2);
    //     let read_lock = recipient
    //         .wallet
    //         .transaction_context
    //         .transaction_metadata_set
    //         .read()
    //         .await;
    //     let wallet_trees = read_lock.witness_trees().unwrap();
    //     let last_leaf = wallet_trees
    //         .witness_tree_orchard
    //         .max_leaf_position(None)
    //         .unwrap();
    //     let server_trees = zingolib::grpc_connector::get_trees(
    //         recipient.get_server_uri(),
    //         recipient.wallet.last_synced_height().await,
    //     )
    //     .await
    //     .unwrap();
    //     let server_orchard_front = zcash_primitives::merkle_tree::read_commitment_tree::<
    //         MerkleHashOrchard,
    //         &[u8],
    //         { zingolib::wallet::data::COMMITMENT_TREE_LEVELS },
    //     >(&hex::decode(server_trees.orchard_tree).unwrap()[..])
    //     .unwrap()
    //     .to_frontier()
    //     .take();
    //     let mut server_orchard_shardtree: ShardTree<_, COMMITMENT_TREE_LEVELS, MAX_SHARD_LEVEL> =
    //         ShardTree::new(
    //             MemoryShardStore::<MerkleHashOrchard, BlockHeight>::empty(),
    //             MAX_REORG,
    //         );
    //     server_orchard_shardtree
    //         .insert_frontier_nodes(
    //             server_orchard_front.unwrap(),
    //             zingolib::testutils::incrementalmerkletree::Retention::Marked,
    //         )
    //         .unwrap();
    //     // This height doesn't matter, all we need is any arbitrary checkpoint ID
    //     // as witness_at_checkpoint_depth requires a checkpoint to function now
    //     server_orchard_shardtree
    //         .checkpoint(BlockHeight::from_u32(0))
    //         .unwrap();
    //     assert_eq!(
    //         wallet_trees
    //             .witness_tree_orchard
    //             .witness_at_checkpoint_depth(last_leaf.unwrap(), 0)
    //             .unwrap_or_else(|_| panic!("{:#?}", wallet_trees.witness_tree_orchard)),
    //         server_orchard_shardtree
    //             .witness_at_checkpoint_depth(last_leaf.unwrap(), 0)
    //             .unwrap()
    //     )
    // }

    #[tokio::test]
    async fn create_send_to_self_with_zfz_active() {
        let (_regtest_manager, _cph, _faucet, recipient, _txid) =
            scenarios::faucet_funded_recipient_default(5_000_000).await;

        recipient
            .propose_send_all(
                address_from_str(&get_base_address_macro!(&recipient, "unified")).unwrap(),
                true,
                None,
            )
            .await
            .unwrap();

        recipient
            .complete_and_broadcast_stored_proposal()
            .await
            .unwrap();

        let value_transfers = &recipient.sorted_value_transfers(true).await;

        assert!(value_transfers.iter().any(|vt| vt.kind()
            == ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                SelfSendValueTransfer::Basic
            ))));
        assert!(value_transfers.iter().any(|vt| vt.kind()
            == ValueTransferKind::Sent(SentValueTransfer::Send)
            && vt.recipient_address() == Some(ZENNIES_FOR_ZINGO_REGTEST_ADDRESS)));
    }

    /// This tests checks that messages_containing returns an empty vector when empty memos are included.
    #[tokio::test]
    async fn filter_empty_messages() {
        let mut environment = LibtonodeEnvironment::setup().await;

        let mut faucet = environment.create_faucet().await;
        let mut recipient = environment.create_client();

        environment.bump_chain().await;
        faucet.sync_and_await(true).await.unwrap();

        check_client_balances!(faucet, o: 0 s: 2_500_000_000u64 t: 0u64);

        from_inputs::quick_send(
            &faucet,
            vec![
                (
                    get_base_address_macro!(recipient, "unified").as_str(),
                    5_000,
                    Some(""),
                ),
                (
                    get_base_address_macro!(recipient, "unified").as_str(),
                    5_000,
                    Some(""),
                ),
            ],
        )
        .await
        .unwrap();

        environment.bump_chain().await;
        recipient.sync_and_await(true).await.unwrap();

        let no_messages = &recipient.messages_containing(None).await;

        assert_eq!(no_messages.len(), 0);

        from_inputs::quick_send(
            &faucet,
            vec![
                (
                    get_base_address_macro!(recipient, "unified").as_str(),
                    5_000,
                    Some("Hello"),
                ),
                (
                    get_base_address_macro!(recipient, "unified").as_str(),
                    5_000,
                    Some(""),
                ),
            ],
        )
        .await
        .unwrap();

        environment.bump_chain().await;
        recipient.sync_and_await(true).await.unwrap();

        let single_message = &recipient.messages_containing(None).await;

        assert_eq!(single_message.len(), 1);
    }

    /// Test sending and receiving messages between three parties.
    ///
    /// This test case consists of the following sequence of events:
    ///
    /// 1. Alice sends a message to Bob.
    /// 2. Alice sends another message to Bob.
    /// 3. Bob sends a message to Alice.
    /// 4. Alice sends a message to Charlie.
    /// 5. Charlie sends a message to Alice.
    ///
    /// After the messages are sent, the test checks that the `messages_containing` method
    /// returns the expected messages for each party in the correct order.
    #[tokio::test]
    async fn message_thread() {
        // Begin test setup
        let (regtest_manager, _cph, mut faucet, mut recipient, _txid) =
            scenarios::faucet_funded_recipient_default(10_000_000).await;
        macro_rules! send_and_sync {
            ($client:ident, $message:ident) => {
                // Propose sending the message
                $client.propose_send($message.clone()).await.unwrap();
                // Complete and broadcast the stored proposal
                $client
                    .complete_and_broadcast_stored_proposal()
                    .await
                    .unwrap();
                // Increase the height and wait for the client
                increase_height_and_wait_for_client(&regtest_manager, &mut $client, 1)
                    .await
                    .unwrap();
            };
        }
        // Addresses: alice, bob, charlie
        let alice = get_base_address(&recipient, PoolType::ORCHARD).await;
        let bob = faucet
            .wallet
            .lock()
            .await
            .generate_unified_address(ReceiverSelection {
                orchard: true,
                sapling: true,
                transparent: true,
            })
            .unwrap();
        let charlie = faucet
            .wallet
            .lock()
            .await
            .generate_unified_address(ReceiverSelection {
                orchard: true,
                sapling: true,
                transparent: true,
            })
            .unwrap();

        // messages
        let alice_to_bob = TransactionRequest::new(vec![Payment::new(
            ZcashAddress::from_str(&bob.encode(&faucet.config().chain)).unwrap(),
            NonNegativeAmount::from_u64(1_000).unwrap(),
            Some(Memo::encode(
                &Memo::from_str(&("Alice->Bob #1\nReply to\n".to_string() + &alice)).unwrap(),
            )),
            None,
            None,
            vec![],
        )
        .unwrap()])
        .unwrap();
        let alice_to_bob_2 = TransactionRequest::new(vec![Payment::new(
            ZcashAddress::from_str(&bob.encode(&faucet.config().chain)).unwrap(),
            NonNegativeAmount::from_u64(1_000).unwrap(),
            Some(Memo::encode(
                &Memo::from_str(&("Alice->Bob #2\nReply to\n".to_string() + &alice)).unwrap(),
            )),
            None,
            None,
            vec![],
        )
        .unwrap()])
        .unwrap();
        let alice_to_charlie = TransactionRequest::new(vec![Payment::new(
            ZcashAddress::from_str(&charlie.encode(&faucet.config().chain)).unwrap(),
            NonNegativeAmount::from_u64(1_000).unwrap(),
            Some(Memo::encode(
                &Memo::from_str(&("Alice->Charlie #2\nReply to\n".to_string() + &alice)).unwrap(),
            )),
            None,
            None,
            vec![],
        )
        .unwrap()])
        .unwrap();
        let charlie_to_alice = TransactionRequest::new(vec![Payment::new(
            ZcashAddress::from_str(&alice).unwrap(),
            NonNegativeAmount::from_u64(1_000).unwrap(),
            Some(Memo::encode(
                &Memo::from_str(
                    &("Charlie->Alice #2\nReply to\n".to_string()
                        + &charlie.encode(&faucet.config().chain)),
                )
                .unwrap(),
            )),
            None,
            None,
            vec![],
        )
        .unwrap()])
        .unwrap();
        let bob_to_alice = TransactionRequest::new(vec![Payment::new(
            ZcashAddress::from_str(&alice).unwrap(),
            NonNegativeAmount::from_u64(1_000).unwrap(),
            Some(Memo::encode(
                &Memo::from_str(
                    &("Bob->Alice #2\nReply to\n".to_string()
                        + &bob.encode(&faucet.config().chain)),
                )
                .unwrap(),
            )),
            None,
            None,
            vec![],
        )
        .unwrap()])
        .unwrap();
        // Complete test setup

        // Message Sending
        send_and_sync!(recipient, alice_to_bob);
        send_and_sync!(recipient, alice_to_bob_2);
        send_and_sync!(faucet, bob_to_alice);
        send_and_sync!(recipient, alice_to_charlie);
        send_and_sync!(faucet, charlie_to_alice);
        // Final sync of recipient
        increase_height_and_wait_for_client(&regtest_manager, &mut recipient, 1)
            .await
            .unwrap();

        // Collect observations
        let value_transfers_bob = &recipient
            .messages_containing(Some(&bob.encode(&recipient.config().chain)))
            .await;
        let value_transfers_charlie = &recipient
            .messages_containing(Some(&charlie.encode(&recipient.config().chain)))
            .await;
        let all_vts = &recipient.sorted_value_transfers(true).await;
        let all_messages = &recipient.messages_containing(None).await;

        // Make assertions
        assert_eq!(value_transfers_bob.len(), 3);
        assert_eq!(value_transfers_charlie.len(), 2);

        // Also asserting the order now (sorry juanky)
        // ALL MESSAGES (First one should be the oldest one)
        assert!(all_messages
            .windows(2)
            .all(|pair| { pair[0].blockheight() <= pair[1].blockheight() }));
        // ALL VTS (First one should be the most recent one)
        assert!(all_vts
            .windows(2)
            .all(|pair| { pair[0].blockheight() >= pair[1].blockheight() }));
    }

    /// Tests that value transfers are properly sorted by block height and index.
    /// It also tests that retrieving the value transfers multiple times in a row returns the same results.
    #[tokio::test]
    async fn value_transfers() {
        let mut environment = LibtonodeEnvironment::setup().await;

        let mut faucet = environment.create_faucet().await;
        let mut recipient = environment.create_client();

        environment.bump_chain().await;
        faucet.sync_and_await(true).await.unwrap();

        check_client_balances!(faucet, o: 0 s: 2_500_000_000u64 t: 0u64);

        from_inputs::quick_send(
            &faucet,
            vec![
                (
                    get_base_address_macro!(recipient, "unified").as_str(),
                    5_000,
                    Some("Message #1"),
                ),
                (
                    get_base_address_macro!(recipient, "unified").as_str(),
                    5_000,
                    Some("Message #2"),
                ),
                (
                    get_base_address_macro!(recipient, "unified").as_str(),
                    5_000,
                    Some("Message #3"),
                ),
                (
                    get_base_address_macro!(recipient, "unified").as_str(),
                    5_000,
                    Some("Message #4"),
                ),
            ],
        )
        .await
        .unwrap();

        environment.bump_chain().await;
        recipient.sync_and_await(true).await.unwrap();

        let value_transfers = &recipient.sorted_value_transfers(true).await;
        let value_transfers1 = &recipient.sorted_value_transfers(true).await;
        let value_transfers2 = &recipient.sorted_value_transfers(true).await;
        let mut value_transfers3 = recipient.sorted_value_transfers(false).await;
        let mut value_transfers4 = recipient.sorted_value_transfers(false).await;

        assert_eq!(value_transfers[0].memos().len(), 4);

        value_transfers3.reverse();
        value_transfers4.reverse();

        assert_eq!(value_transfers, value_transfers1);
        assert_eq!(value_transfers, value_transfers2);
        assert_eq!(value_transfers, &value_transfers3);
        assert_eq!(value_transfers, &value_transfers4);
    }

    pub mod tex {
        use zcash_primitives::transaction::TxId;
        use zingolib::{utils, wallet::LightWallet};

        use super::*;

        fn first_taddr_to_tex(wallet: &LightWallet) -> ZcashAddress {
            let taddr = ZcashAddress::try_from_encoded(
                &wallet.get_first_address(PoolType::Transparent).unwrap(),
            )
            .unwrap();

            let zcash_address::AddressKind::P2pkh(taddr_bytes) = taddr.kind() else {
                panic!()
            };
            let tex_string = utils::interpret_taddr_as_tex_addr(*taddr_bytes, &wallet.network);
            //            let tex_string = utils::interpret_taddr_as_tex_addr(*taddr_bytes);

            ZcashAddress::try_from_encoded(&tex_string).unwrap()
        }
        #[tokio::test]
        async fn send_to_tex() {
            let (ref regtest_manager, _cph, ref faucet, mut sender, _txid) =
                scenarios::faucet_funded_recipient_default(5_000_000).await;

            let tex_addr_from_first = first_taddr_to_tex(&*faucet.wallet.lock().await);
            let payment = vec![Payment::without_memo(
                tex_addr_from_first.clone(),
                NonNegativeAmount::from_u64(100_000).unwrap(),
            )];

            let transaction_request = TransactionRequest::new(payment).unwrap();

            let proposal = sender.propose_send(transaction_request).await.unwrap();
            assert_eq!(proposal.steps().len(), 2usize);
            let _sent_txids_according_to_broadcast = sender
                .complete_and_broadcast_stored_proposal()
                .await
                .unwrap();
            let _txids = sender
                .wallet
                .lock()
                .await
                .wallet_transactions
                .keys()
                .cloned()
                .collect::<Vec<TxId>>();
            increase_height_and_wait_for_client(regtest_manager, &mut sender, 1)
                .await
                .unwrap();
            assert_eq!(sender.wallet.lock().await.wallet_transactions.len(), 3usize);

            // FIXME: add tex addresses to encoded memos
            // let val_tranfers = sender.sorted_value_transfers(true).await;
            // assert_eq!(
            //     val_tranfers[0].recipient_address().unwrap(),
            //     tex_addr_from_first.encode()
            // );
        }
    }

    #[tokio::test]
    async fn received_tx_status_pending_to_confirmed_with_mempool_monitor() {
        tracing_subscriber::fmt().init();

        let (regtest_manager, _cph, faucet, mut recipient, _txid) =
            scenarios::faucet_funded_recipient_default(100_000).await;

        from_inputs::quick_send(
            &faucet,
            vec![(
                &get_base_address_macro!(&recipient, "unified"),
                // &get_base_address_macro!(&recipient, "sapling"),
                20_000,
                None,
            )],
        )
        .await
        .unwrap();

        recipient.sync_and_await(true).await.unwrap();

        let transactions = &recipient.transaction_summaries().await.0;
        transactions.iter().for_each(|tx| {
            dbg!(tx);
        });
        assert_eq!(
            transactions
                .iter()
                .find(|tx| tx.value() == 20_000)
                .unwrap()
                .status(),
            ConfirmationStatus::Mempool(BlockHeight::from_u32(6))
        );

        increase_height_and_wait_for_client(&regtest_manager, &mut recipient, 1)
            .await
            .unwrap();

        let transactions = &recipient.transaction_summaries().await.0;
        assert_eq!(
            transactions
                .iter()
                .find(|tx| tx.value() == 20_000)
                .unwrap()
                .status(),
            ConfirmationStatus::Confirmed(BlockHeight::from_u32(6))
        );
    }

    #[tokio::test]
    async fn utxos_are_not_prematurely_confirmed() {
        let (regtest_manager, _cph, faucet, mut recipient) =
            scenarios::faucet_recipient_default().await;
        from_inputs::quick_send(
            &faucet,
            vec![(
                &get_base_address_macro!(recipient, "transparent"),
                100_000,
                None,
            )],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&regtest_manager, &mut recipient, 1)
            .await
            .unwrap();
        let wallet = recipient.wallet.lock().await;
        let preshield_utxos = wallet
            .wallet_outputs::<TransparentCoin>()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(preshield_utxos.len(), 1);
        assert!(wallet
            .output_spend_status(preshield_utxos.first().unwrap())
            .is_unspent());
        drop(wallet);

        recipient.quick_shield().await.unwrap();
        increase_height_and_wait_for_client(&regtest_manager, &mut recipient, 1)
            .await
            .unwrap();

        let wallet = recipient.wallet.lock().await;
        let postshield_utxos = wallet.wallet_outputs::<TransparentCoin>();
        assert_eq!(postshield_utxos.len(), 1);
        assert!(wallet
            .output_spend_status(*postshield_utxos.first().unwrap())
            .is_confirmed_spent());
        assert_eq!(
            preshield_utxos.first().unwrap().output_id(),
            postshield_utxos.first().unwrap().output_id(),
        );
    }

    #[tokio::test]
    async fn zcashd_sapling_commitment_tree() {
        //  TODO:  Make this test assert something, what is this a test of?
        //  TODO:  Add doc-comment explaining what constraints this test
        //  enforces
        let (regtest_manager, _cph, _faucet) = scenarios::faucet_default().await;
        let trees = regtest_manager
            .get_cli_handle()
            .args(["z_gettreestate", "1"])
            .output()
            .expect("Couldn't get the trees.");
        let trees = json::parse(&String::from_utf8_lossy(&trees.stdout));
        let pretty_trees = json::stringify_pretty(trees.unwrap(), 4);
        println!("{}", pretty_trees);
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
        let (regtest_manager, _cph, _client) = scenarios::unfunded_client_default().await;
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
    }

    #[tokio::test]
    async fn diversified_addresses_receive_funds_in_best_pool() {
        let (regtest_manager, _cph, faucet, mut recipient) =
            scenarios::faucet_recipient_default().await;
        for code in ["o", "zo", "z"] {
            recipient.do_new_address(code).await.unwrap();
        }
        let addresses = recipient.do_addresses(UAReceivers::All).await;
        let address_5000_nonememo_tuples = addresses
            .members()
            .map(|ua| (ua["address"].as_str().unwrap(), 5_000, None))
            .collect::<Vec<(&str, u64, Option<&str>)>>();
        from_inputs::quick_send(&faucet, address_5000_nonememo_tuples)
            .await
            .unwrap();
        zingolib::testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &mut recipient,
            1,
        )
        .await
        .unwrap();
        let balance_b = recipient.do_balance().await;
        assert_eq!(
            balance_b,
            PoolBalances {
                sapling_balance: Some(5000),
                verified_sapling_balance: Some(5000),
                spendable_sapling_balance: Some(5000),
                unverified_sapling_balance: Some(0),
                orchard_balance: Some(15000),
                verified_orchard_balance: Some(15000),
                spendable_orchard_balance: Some(15000),
                unverified_orchard_balance: Some(0),
                transparent_balance: Some(0)
            }
        );
    }

    #[tokio::test]
    async fn diversification_deterministic_and_coherent() {
        let (_regtest_manager, _cph, mut client_builder, regtest_network) =
            scenarios::custom_clients_default().await;
        let seed_phrase = Mnemonic::<bip0039::English>::from_entropy([1; 32])
            .unwrap()
            .to_string();
        let mut recipient1 = client_builder.build_client(seed_phrase, 0, false, regtest_network);
        let base_transparent_receiver = "tmS9nbexug7uT8x1cMTLP1ABEyKXpMjR5F1";
        assert_eq!(
            &get_base_address_macro!(recipient1, "transparent"),
            &base_transparent_receiver
        );
        let base_sapling_receiver = "\
        zregtestsapling1lhjvuj4s3ghhccnjaefdzuwp3h3mfluz6tm8h0dsq2ym3f77zsv0wrrszpmaqlezm3kt6ajdvlw";
        assert_eq!(
            &get_base_address_macro!(recipient1, "sapling"),
            &base_sapling_receiver
        );
        // Verify that the provided seed generates the expected uregtest1qtqr46..  unified address (UA)
        let base_unified_address = "\
        uregtest1qtqr46fwkhmdn336uuyvvxyrv0l7trgc0z9clpryx6vtladnpyt4wvq99p59f4rcyuvpmmd0hm4k5vv6j8\
        edj6n8ltk45sdkptlk7rtzlm4uup4laq8ka8vtxzqemj3yhk6hqhuypupzryhv66w65lah9ms03xa8nref7gux2zzhj\
        nfanxnnrnwscmz6szv2ghrurhu3jsqdx25y2yh";
        assert_eq!(
            &get_base_address_macro!(recipient1, "unified"),
            &base_unified_address
        );

        //Verify that 1 increment of diversification with a tz receiver set produces uregtest1m8un60u... UA
        let new_address = recipient1.do_new_address("tzo").await.unwrap();
        let ua_index_1 = recipient1.do_addresses(UAReceivers::All).await[1].clone();
        let ua_address_index_1 = ua_index_1["address"].clone().to_string();
        assert_eq!(&new_address[0].to_string(), &ua_address_index_1);
        let sapling_index_1 = ua_index_1["receivers"]["sapling"].clone().to_string();
        let transparent_index_1 = ua_index_1["receivers"]["transparent"].clone().to_string();
        let ua_address_index_1_match = ua_address_index_1
            == "\
            uregtest1yhu9ke9hung002w5vcez7y6fe7sgqe4rnc3l2tqyz3yqctmtays6peukkhj2lx45urq666h4dpduz0\
            rjzlmky7cuayj285d003futaljg355tz94l6xnklk5kgthe2x942s3qkxedypsadla56fjx4e5nca9672jmxekj\
            pp94ahz0ax963r2v9wwxfzadnzt3fgwa8pytdhcy4l6z0h";
        let sapling_index_1_match = sapling_index_1
        == "zregtestsapling14wl6gy5h2tg528znyrqayfh2sekntk3lvmwsw68wjz2g205t62sv5xeyzvfk4hlxdwd9gh4ws9n";
        let transparent_index_1_match =
            transparent_index_1 == "tmQuMoTTjU3GFfTjrhPiBYihbTVfYmPk5Gr";

        //  Show orchard diversification is working (regardless of other diversifiers, both previous and other-pool).
        let new_orchard_only_address = recipient1.do_new_address("o").await.unwrap();
        let ua_address_index_2 = new_orchard_only_address[0].to_string();
        let ua_2_orchard_match = ua_address_index_2 ==  "\
        uregtest1yyw480060mdzvnfpfayfhackhgh0jjsuq5lfjf9u68hulmn9efdalmz583xlq6pt8lmyylky6p2usx57lfv7tqu9j0tqqs8asq25p49n";
        assert!(
            ua_address_index_1_match && sapling_index_1_match && transparent_index_1_match,
            "\n\
            ua_1, match: {} Observed:\n\
            {}\n\n\
            sapling_1, match: {} Observed:\n\
            {}\n\n\
            transparent_1, match: {} Observed:\n\
            {}\n\n\
            ua_address_index_2, match: {} Observed:\n\
            {}\n
        ",
            ua_address_index_1_match,
            ua_address_index_1,
            sapling_index_1_match,
            sapling_index_1,
            transparent_index_1_match,
            transparent_index_1,
            ua_2_orchard_match,
            ua_address_index_2
        );
    }

    #[tokio::test]
    async fn ensure_taddrs_from_old_seeds_work() {
        let (_regtest_manager, _cph, mut client_builder, regtest_network) =
            scenarios::custom_clients_default().await;
        // The first taddr generated on commit 9e71a14eb424631372fd08503b1bd83ea763c7fb
        let transparent_address = "tmFLszfkjgim4zoUMAXpuohnFBAKy99rr2i";

        let client_b = client_builder.build_client(
            HOSPITAL_MUSEUM_SEED.to_string(),
            0,
            false,
            regtest_network,
        );

        assert_eq!(
            get_base_address_macro!(client_b, "transparent"),
            transparent_address
        );
    }

    #[tokio::test]
    async fn sync_all_epochs_from_sapling() {
        let regtest_network = RegtestNetwork::new(1, 1, 3, 5, 7, 9, 11);
        let (regtest_manager, _cph, mut lightclient) =
            scenarios::unfunded_client(regtest_network, true).await;
        increase_height_and_wait_for_client(&regtest_manager, &mut lightclient, 14)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn mine_to_orchard() {
        tracing_subscriber::fmt().init();

        let regtest_network = RegtestNetwork::all_upgrades_active();
        let (regtest_manager, _cph, mut faucet) = scenarios::faucet(
            PoolType::Shielded(ShieldedProtocol::Orchard),
            regtest_network,
            true,
        )
        .await;
        check_client_balances!(faucet, o: 1_875_000_000 s: 0 t: 0);
        increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 1)
            .await
            .unwrap();
        check_client_balances!(faucet, o: 2_500_000_000u64 s: 0 t: 0);
    }

    #[tokio::test]
    async fn mine_to_sapling() {
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let (regtest_manager, _cph, mut faucet) = scenarios::faucet(
            PoolType::Shielded(ShieldedProtocol::Sapling),
            regtest_network,
            true,
        )
        .await;
        check_client_balances!(faucet, o: 0 s: 1_875_000_000 t: 0);
        increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 1)
            .await
            .unwrap();
        check_client_balances!(faucet, o: 0 s: 2_500_000_000u64 t: 0);
    }

    #[tokio::test]
    async fn mine_to_transparent() {
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let (regtest_manager, _cph, mut faucet, _recipient) =
            scenarios::faucet_recipient(PoolType::Transparent, regtest_network, true).await;
        check_client_balances!(faucet, o: 0 s: 0 t: 1_875_000_000);
        increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 1)
            .await
            .unwrap();
        check_client_balances!(faucet, o: 0 s: 0 t: 2_500_000_000u64);
    }

    // test fails to exit when syncing pre-sapling
    // possible issue with dropping child process handler?
    #[ignore]
    #[tokio::test]
    async fn sync_all_epochs() {
        let regtest_network = RegtestNetwork::new(1, 3, 5, 7, 9, 11, 13);
        let (regtest_manager, _cph, mut lightclient) =
            scenarios::unfunded_client(regtest_network, true).await;
        increase_height_and_wait_for_client(&regtest_manager, &mut lightclient, 14)
            .await
            .unwrap();
    }

    // test fails with error message: "66: tx unpaid action limit exceeded"
    #[ignore]
    #[tokio::test]
    async fn mine_to_transparent_and_shield() {
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let (regtest_manager, _cph, mut faucet, _recipient) =
            scenarios::faucet_recipient(PoolType::Transparent, regtest_network, true).await;
        increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 100)
            .await
            .unwrap();
        faucet.quick_shield().await.unwrap();
    }
    #[tokio::test]
    async fn mine_to_transparent_and_propose_shielding() {
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let (regtest_manager, _cph, mut faucet, _recipient) =
            scenarios::faucet_recipient(PoolType::Transparent, regtest_network, true).await;
        increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 100)
            .await
            .unwrap();
        let proposal = faucet.propose_shield().await.unwrap();
        let only_step = proposal.steps().first();

        // Orchard action and dummy, plus 4 transparent inputs
        let expected_fee = 30_000;

        assert_eq!(proposal.steps().len(), 1);
        assert_eq!(only_step.transparent_inputs().len(), 4);
        assert_eq!(
            only_step.balance().fee_required(),
            NonNegativeAmount::const_from_u64(expected_fee)
        );
        // Only one change item. I guess change could be split between pools?
        assert_eq!(only_step.balance().proposed_change().len(), 1);
        assert_eq!(
            only_step
                .balance()
                .proposed_change()
                .first()
                .unwrap()
                .value(),
            NonNegativeAmount::const_from_u64((block_rewards::CANOPY * 4) - expected_fee)
        )
    }
    #[tokio::test]
    async fn mine_to_transparent_and_propose_shielding_with_div_addr() {
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let (regtest_manager, _cph, mut faucet, _recipient) =
            scenarios::faucet_recipient(PoolType::Transparent, regtest_network, true).await;
        increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 100)
            .await
            .unwrap();
        faucet.do_new_address("zto").await.unwrap();
        let proposal = faucet.propose_shield().await.unwrap();
        let only_step = proposal.steps().first();

        // Orchard action and dummy, plus 4 transparent inputs
        let expected_fee = 30_000;

        assert_eq!(proposal.steps().len(), 1);
        assert_eq!(only_step.transparent_inputs().len(), 4);
        assert_eq!(
            only_step.balance().fee_required(),
            NonNegativeAmount::const_from_u64(expected_fee)
        );
        // Only one change item. I guess change could be split between pools?
        assert_eq!(only_step.balance().proposed_change().len(), 1);
        assert_eq!(
            only_step
                .balance()
                .proposed_change()
                .first()
                .unwrap()
                .value(),
            NonNegativeAmount::const_from_u64((block_rewards::CANOPY * 4) - expected_fee)
        )
    }
}
mod slow {
    use pepper_sync::wallet::{
        NoteInterface, OrchardNote, OutgoingNoteInterface, OutputInterface, SaplingNote,
        TransparentCoin,
    };
    use shardtree::store::ShardStore;
    use testvectors::TEST_TXID;
    use zcash_client_backend::{PoolType, ShieldedProtocol};
    use zcash_primitives::consensus::BlockHeight;
    use zcash_primitives::memo::Memo;
    use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;
    use zcash_primitives::zip32::{self, AccountId};
    use zingo_status::confirmation_status::ConfirmationStatus;
    use zingolib::config::ChainType;
    use zingolib::lightclient::describe::UAReceivers;
    use zingolib::lightclient::send::send_with_proposal::{
        CompleteAndBroadcastError, QuickSendError,
    };
    use zingolib::testutils::lightclient::{from_inputs, get_fees_paid_by_client};
    use zingolib::testutils::{
        assert_transaction_summary_equality, assert_transaction_summary_exists, build_fvk_client,
    };
    use zingolib::utils;
    use zingolib::utils::conversion::txid_from_hex_encoded_str;
    use zingolib::wallet::data::summaries::{
        BasicNoteSummary, OutgoingNoteSummary, TransactionSummaryBuilder,
        TransactionSummaryInterface,
    };
    use zingolib::wallet::output::SpendStatus;
    use zingolib::wallet::send::BuildTransactionError;
    use zingolib::wallet::summary::{self, SendType, TransactionKind};

    use super::*;

    #[tokio::test]
    async fn zero_value_receipts() {
        let (regtest_manager, _cph, faucet, mut recipient, _txid) =
            scenarios::faucet_funded_recipient_default(100_000).await;

        let sent_value = 0;
        let _sent_transaction_id = from_inputs::quick_send(
            &faucet,
            vec![(
                &get_base_address_macro!(recipient, "unified"),
                sent_value,
                None,
            )],
        )
        .await
        .unwrap();

        zingolib::testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &mut recipient,
            5,
        )
        .await
        .unwrap();
        let _sent_transaction_id = from_inputs::quick_send(
            &recipient,
            vec![(&get_base_address_macro!(faucet, "unified"), 1000, None)],
        )
        .await
        .unwrap();
        zingolib::testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &mut recipient,
            5,
        )
        .await
        .unwrap();

        println!(
            "{}",
            serde_json::to_string_pretty(&recipient.do_balance().await).unwrap()
        );
        println!(
            "{}",
            JsonValue::from(recipient.sorted_value_transfers(true).await).pretty(4)
        );
    }
    #[tokio::test]
    async fn zero_value_change() {
        let value = 100_000;
        let (regtest_manager, _cph, faucet, mut recipient, _txid) =
            scenarios::faucet_funded_recipient_default(value).await;

        let sent_value = value - u64::from(MINIMUM_FEE);
        let sent_transaction_id = from_inputs::quick_send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "unified"),
                sent_value,
                None,
            )],
        )
        .await
        .unwrap()
        .first()
        .to_string();

        zingolib::testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &mut recipient,
            5,
        )
        .await
        .unwrap();

        let recipient_wallet = recipient.wallet.lock().await;
        let transparent_coins = recipient_wallet.wallet_outputs::<TransparentCoin>();
        assert_eq!(transparent_coins.len(), 0);
        let sapling_notes = recipient_wallet.wallet_outputs::<SaplingNote>();
        assert_eq!(sapling_notes.len(), 0);
        let orchard_notes = recipient_wallet.wallet_outputs::<OrchardNote>();
        let unspent_orchard_notes = orchard_notes
            .iter()
            .filter(|&&note| recipient_wallet.output_spend_status(note).is_unspent())
            .collect::<Vec<_>>();
        let spent_orchard_notes = orchard_notes
            .iter()
            .filter(|&&note| {
                recipient_wallet
                    .output_spend_status(note)
                    .is_confirmed_spent()
            })
            .collect::<Vec<_>>();

        assert_eq!(unspent_orchard_notes.len(), 1);
        assert_eq!(
            orchard_notes
                .iter()
                .filter(|&&note| recipient_wallet
                    .output_spend_status(note)
                    .is_pending_spent())
                .count(),
            0
        );
        assert_eq!(spent_orchard_notes.len(), 1);

        assert_eq!(unspent_orchard_notes.first().unwrap().value(), 0);
        assert_eq!(
            spent_orchard_notes
                .first()
                .unwrap()
                .spending_transaction()
                .unwrap()
                .to_string(),
            sent_transaction_id
        );
        drop(recipient_wallet);

        check_client_balances!(recipient, o: 0 s: 0 t: 0);
    }
    // FIXME: zingo2
    // #[tokio::test]
    // async fn witness_clearing() {
    //     let (regtest_manager, _cph, faucet, recipient, txid) =
    //         scenarios::faucet_funded_recipient_default(100_000).await;
    //     let txid = utils::conversion::txid_from_hex_encoded_str(&txid).unwrap();

    //     // 3. Send z-to-z transaction to external z address with a memo
    //     let sent_value = 2000;
    //     let outgoing_memo = "Outgoing Memo";

    //     let faucet_ua = get_base_address_macro!(faucet, "unified");

    //     let _sent_transaction_id = from_inputs::quick_send(
    //         &recipient,
    //         vec![(&faucet_ua, sent_value, Some(outgoing_memo))],
    //     )
    //     .await
    //     .unwrap();

    //     for txid_known in recipient
    //         .wallet
    //         .transactions()
    //         .read()
    //         .await
    //         .transaction_records_by_id
    //         .keys()
    //     {
    //         dbg!(txid_known);
    //     }

    //     // transaction is not yet mined, so witnesses should still be there
    //     let position = recipient
    //         .wallet
    //         .transactions()
    //         .read()
    //         .await
    //         .transaction_records_by_id
    //         .get(&txid)
    //         .unwrap()
    //         .orchard_notes
    //         .first()
    //         .unwrap()
    //         .witnessed_position
    //         .unwrap();
    //     assert!(recipient
    //         .wallet
    //         .transaction_context
    //         .transaction_metadata_set
    //         .read()
    //         .await
    //         .witness_trees()
    //         .unwrap()
    //         .witness_tree_orchard
    //         .marked_positions()
    //         .unwrap()
    //         .contains(&position));

    //     // 4. Mine the sent transaction
    //     zingolib::testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 1)
    //         .await
    //         .unwrap();

    //     // transaction is now mined, but witnesses should still be there because not 100 blocks yet (i.e., could get reorged)
    //     let position = recipient
    //         .wallet
    //         .transactions()
    //         .read()
    //         .await
    //         .transaction_records_by_id
    //         .get(&txid)
    //         .unwrap()
    //         .orchard_notes
    //         .first()
    //         .unwrap()
    //         .witnessed_position
    //         .unwrap();
    //     assert!(recipient
    //         .wallet
    //         .transaction_context
    //         .transaction_metadata_set
    //         .read()
    //         .await
    //         .witness_trees()
    //         .unwrap()
    //         .witness_tree_orchard
    //         .marked_positions()
    //         .unwrap()
    //         .contains(&position));
    //     dbg!(
    //         &recipient
    //             .wallet
    //             .transaction_context
    //             .transaction_metadata_set
    //             .read()
    //             .await
    //             .witness_trees()
    //             .unwrap()
    //             .witness_tree_orchard
    //     );

    //     // 5. Mine 50 blocks, witness should still be there
    //     zingolib::testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 50)
    //         .await
    //         .unwrap();
    //     let position = recipient
    //         .wallet
    //         .transactions()
    //         .read()
    //         .await
    //         .transaction_records_by_id
    //         .get(&txid)
    //         .unwrap()
    //         .orchard_notes
    //         .first()
    //         .unwrap()
    //         .witnessed_position
    //         .unwrap();
    //     assert!(recipient
    //         .wallet
    //         .transaction_context
    //         .transaction_metadata_set
    //         .read()
    //         .await
    //         .witness_trees()
    //         .unwrap()
    //         .witness_tree_orchard
    //         .marked_positions()
    //         .unwrap()
    //         .contains(&position));

    //     // 5. Mine 100 blocks, witness should now disappear
    //     zingolib::testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 50)
    //         .await
    //         .unwrap();
    //     let position = recipient
    //         .wallet
    //         .transactions()
    //         .read()
    //         .await
    //         .transaction_records_by_id
    //         .get(&txid)
    //         .unwrap()
    //         .orchard_notes
    //         .first()
    //         .unwrap()
    //         .witnessed_position
    //         .unwrap();
    //     //Note: This is a negative assertion. Notice the "!"
    //     dbg!(
    //         &recipient
    //             .wallet
    //             .transaction_context
    //             .transaction_metadata_set
    //             .read()
    //             .await
    //             .witness_trees()
    //             .unwrap()
    //             .witness_tree_orchard
    //     );
    //     assert!(!recipient
    //         .wallet
    //         .transaction_context
    //         .transaction_metadata_set
    //         .read()
    //         .await
    //         .witness_trees()
    //         .unwrap()
    //         .witness_tree_orchard
    //         .marked_positions()
    //         .unwrap()
    //         .contains(&position));
    // }

    #[tokio::test]
    async fn test_scanning_in_watch_only_mode() {
        // # Scenario:
        // 3. reset wallet
        // 4. for every combination of FVKs
        //     4.1. init a wallet with UFVK
        //     4.2. check that the wallet is empty
        //     4.3. rescan
        //     4.4. check that notes and utxos were detected by the wallet

        tracing_subscriber::fmt().init();
        let (regtest_manager, _cph, mut client_builder, regtest_network) =
            scenarios::custom_clients_default().await;
        let mut faucet = client_builder.build_faucet(false, regtest_network);
        let mut original_recipient = client_builder.build_client(
            HOSPITAL_MUSEUM_SEED.to_string(),
            0,
            false,
            regtest_network,
        );
        let zingo_config = zingolib::config::load_clientconfig(
            client_builder.server_id,
            Some(client_builder.zingo_datadir),
            ChainType::Regtest(regtest_network),
        )
        .unwrap();

        let (recipient_taddr, recipient_sapling, recipient_unified) = (
            get_base_address_macro!(original_recipient, "transparent"),
            get_base_address_macro!(original_recipient, "sapling"),
            get_base_address_macro!(original_recipient, "unified"),
        );
        let addr_amount_memos = vec![
            (recipient_taddr.as_str(), 10_000u64, None),
            (recipient_sapling.as_str(), 20_000u64, None),
            (recipient_unified.as_str(), 30_000u64, None),
        ];
        // 1. fill wallet with a coinbase transaction by syncing faucet with 1-block increase
        zingolib::testutils::increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 1)
            .await
            .unwrap();
        // 2. send a transaction containing all types of outputs
        from_inputs::quick_send(&faucet, addr_amount_memos)
            .await
            .unwrap();
        zingolib::testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &mut original_recipient,
            1,
        )
        .await
        .unwrap();
        let original_recipient_balance = original_recipient.do_balance().await;
        let sent_t_value = original_recipient_balance.transparent_balance.unwrap();
        let sent_s_value = original_recipient_balance.sapling_balance.unwrap();
        let sent_o_value = original_recipient_balance.orchard_balance.unwrap();
        assert_eq!(sent_t_value, 10_000u64);
        assert_eq!(sent_s_value, 20_000u64);
        assert_eq!(sent_o_value, 30_000u64);

        // check that do_rescan works
        original_recipient.rescan_and_await(true).await.unwrap();
        check_client_balances!(original_recipient, o: sent_o_value s: sent_s_value t: sent_t_value);

        // Extract viewing keys
        let original_wallet = original_recipient.wallet.lock().await;
        let [o_fvk, s_fvk, t_fvk] = zingolib::testutils::build_fvks_from_unified_keystore(
            &original_wallet.unified_key_store,
        );
        let fvks_sets = [
            vec![&o_fvk],
            vec![&s_fvk],
            vec![&o_fvk, &s_fvk],
            vec![&o_fvk, &t_fvk],
            vec![&s_fvk, &t_fvk],
            vec![&o_fvk, &s_fvk, &t_fvk],
        ];
        for fvks in fvks_sets.iter() {
            tracing::info!("testing UFVK containing:");
            tracing::info!("    orchard fvk: {}", fvks.contains(&&o_fvk));
            tracing::info!("    sapling fvk: {}", fvks.contains(&&s_fvk));
            tracing::info!("    transparent fvk: {}", fvks.contains(&&t_fvk));

            let mut watch_client = build_fvk_client(fvks, zingo_config.clone());
            // assert empty wallet before rescan
            let balance = watch_client.do_balance().await;
            check_expected_balance_with_fvks(fvks, balance, 0, 0, 0);
            watch_client.rescan_and_await(true).await.unwrap();
            let balance = watch_client.do_balance().await;
            {
                let watch_wallet = watch_client.wallet.lock().await;
                let orchard_notes = watch_wallet.note_summaries::<OrchardNote>(true);
                let sapling_notes = watch_wallet.note_summaries::<SaplingNote>(true);
                let transparent_coin = watch_wallet.coin_summaries(true);

                check_view_capability_bounds(
                    &balance,
                    &watch_wallet.unified_key_store,
                    fvks,
                    &o_fvk,
                    &s_fvk,
                    &t_fvk,
                    Some(sent_o_value),
                    Some(sent_s_value),
                    Some(sent_t_value),
                    &orchard_notes,
                    &sapling_notes,
                    &transparent_coin,
                );
            }

            watch_client.rescan_and_await(true).await.unwrap();
            assert!(matches!(
                from_inputs::quick_send(&watch_client, vec![(testvectors::EXT_TADDR, 1000, None)])
                    .await,
                Err(QuickSendError::CompleteAndBroadcast(
                    CompleteAndBroadcastError::BuildTransaction(
                        BuildTransactionError::NoSpendCapability
                    )
                ))
            ));
        }
    }
    #[tokio::test]
    async fn t_incoming_t_outgoing_disallowed() {
        let (regtest_manager, _cph, faucet, mut recipient) =
            scenarios::faucet_recipient_default().await;

        // 2. Get an incoming transaction to a t address
        let recipient_taddr = get_base_address_macro!(recipient, "transparent");
        let value = 100_000;

        from_inputs::quick_send(&faucet, vec![(recipient_taddr.as_str(), value, None)])
            .await
            .unwrap();

        zingolib::testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &mut recipient,
            1,
        )
        .await
        .unwrap();
        recipient.sync_and_await(true).await.unwrap();

        // 3. Test the list
        let transaction = recipient
            .wallet
            .lock()
            .await
            .transaction_summaries()
            .await
            .0
            .first()
            .unwrap()
            .clone();
        assert_eq!(transaction.blockheight(), 4.into());
        assert_eq!(
            recipient.do_addresses(UAReceivers::All).await[0]["receivers"]["transparent"]
                .to_string(),
            recipient_taddr
        );
        assert_eq!(transaction.value(), value);

        // 4. We can't spend the funds, as they're transparent. We need to shield first
        let sent_value = 20_000;
        let sent_transaction_error =
            from_inputs::quick_send(&recipient, vec![(testvectors::EXT_TADDR, sent_value, None)])
                .await
                .unwrap_err();
        assert!(matches!(
            sent_transaction_error,
            QuickSendError::ProposeSend(ProposeSendError::Proposal(
                zcash_client_backend::data_api::error::Error::InsufficientFunds {
                    available: _,
                    required: _
                }
            ))
        ));
    }

    #[tokio::test]
    async fn sends_to_self_handle_balance_properly() {
        let transparent_funding = 100_000;
        let (ref regtest_manager, _cph, faucet, mut recipient) =
            scenarios::faucet_recipient_default().await;
        from_inputs::quick_send(
            &faucet,
            vec![(
                &get_base_address_macro!(recipient, "transparent"),
                transparent_funding,
                None,
            )],
        )
        .await
        .unwrap();
        zingolib::testutils::increase_height_and_wait_for_client(
            regtest_manager,
            &mut recipient,
            1,
        )
        .await
        .unwrap();
        recipient.quick_shield().await.unwrap();
        zingolib::testutils::increase_height_and_wait_for_client(
            regtest_manager,
            &mut recipient,
            1,
        )
        .await
        .unwrap();
        println!(
            "{}",
            serde_json::to_string_pretty(&recipient.do_balance().await).unwrap()
        );
        println!("{}", recipient.transaction_summaries().await);
        println!(
            "{}",
            JsonValue::from(recipient.sorted_value_transfers(true).await).pretty(2)
        );
        recipient.rescan_and_await(true).await.unwrap();
        println!(
            "{}",
            serde_json::to_string_pretty(&recipient.do_balance().await).unwrap()
        );
        println!("{}", recipient.transaction_summaries().await);
        println!(
            "{}",
            JsonValue::from(recipient.sorted_value_transfers(true).await).pretty(2)
        );
        // TODO: Add asserts!
    }
    #[tokio::test]
    async fn send_to_ua_saves_full_ua_in_wallet() {
        let (regtest_manager, _cph, mut faucet, recipient) =
            scenarios::faucet_recipient_default().await;
        //utils::increase_height_and_wait_for_client(&regtest_manager, &faucet, 5).await;
        let recipient_unified_address = get_base_address_macro!(recipient, "unified");
        let sent_value = 50_000;
        from_inputs::quick_send(
            &faucet,
            vec![(recipient_unified_address.as_str(), sent_value, None)],
        )
        .await
        .unwrap();
        zingolib::testutils::increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 1)
            .await
            .unwrap();
        let transactions = faucet.transaction_summaries().await.0;
        assert!(transactions.iter().any(|transaction| {
            transaction
                .outgoing_orchard_notes()
                .iter()
                .chain(transaction.outgoing_sapling_notes().iter())
                .any(|note| {
                    note.recipient_unified_address == Some(recipient_unified_address.clone())
                })
        }));
        faucet.rescan_and_await(true).await.unwrap();
        let rescanned_transactions = faucet.transaction_summaries().await.0;
        assert!(rescanned_transactions.iter().any(|transaction| {
            transaction
                .outgoing_orchard_notes()
                .iter()
                .chain(transaction.outgoing_sapling_notes().iter())
                .any(|note| {
                    note.recipient_unified_address == Some(recipient_unified_address.clone())
                })
        }));
        assert_eq!(
            transactions,
            rescanned_transactions,
            "Pre-Rescan: {}\n\n\nPost-Rescan: {}\n\n\n",
            json::stringify_pretty(transactions.clone(), 4),
            json::stringify_pretty(rescanned_transactions.clone(), 4)
        );
    }
    #[tokio::test]
    async fn send_to_transparent_and_sapling_maintain_balance() {
        // Receipt of orchard funds
        let recipient_initial_funds = 100_000_000;
        let (ref regtest_manager, _cph, mut faucet, mut recipient, _txid) =
            scenarios::faucet_funded_recipient_default(recipient_initial_funds).await;

        let summary_orchard_receipt = TransactionSummaryBuilder::new()
            .blockheight(BlockHeight::from_u32(5))
            .status(ConfirmationStatus::Confirmed(BlockHeight::from_u32(5)))
            .datetime(0)
            .txid(utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap())
            .value(recipient_initial_funds)
            .zec_price(None)
            .kind(TransactionKind::Received)
            .fee(Some(10_000))
            .orchard_notes(vec![BasicNoteSummary::from_parts(
                recipient_initial_funds,
                SpendStatus::Spent(
                    utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
                ),
                0,
                None,
            )])
            .sapling_notes(vec![])
            .transparent_coins(vec![])
            .outgoing_orchard_notes(vec![])
            .outgoing_sapling_notes(vec![])
            .build()
            .unwrap();

        // Send to faucet (external) sapling
        let first_send_to_sapling = 20_000;
        from_inputs::quick_send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "sapling"),
                first_send_to_sapling,
                None,
            )],
        )
        .await
        .unwrap();
        zingolib::testutils::increase_height_and_wait_for_client(
            regtest_manager,
            &mut recipient,
            1,
        )
        .await
        .unwrap();
        let summary_external_sapling = TransactionSummaryBuilder::new()
            .blockheight(BlockHeight::from_u32(6))
            .status(ConfirmationStatus::Confirmed(BlockHeight::from_u32(6)))
            .datetime(0)
            .txid(utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap())
            .value(first_send_to_sapling)
            .zec_price(None)
            .kind(TransactionKind::Sent(SendType::Send))
            .fee(Some(20_000))
            .orchard_notes(vec![BasicNoteSummary::from_parts(
                99_960_000,
                SpendStatus::TransmittedSpent(
                    utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
                ),
                0,
                None,
            )])
            .sapling_notes(vec![])
            .transparent_coins(vec![])
            .outgoing_orchard_notes(vec![OutgoingNoteSummary {
                 value: 99_960_000,
                 memo: None,
                 recipient: "uregtest1ue949txhf9t2z6ldg8wc6s5t439t2hu55yh9l58gc23cmxthths836nxtpyvhpkrftsp2jnnp9eadtqy2nefxn04eyxeu8l0x5kk8ct9".to_string(),
                 recipient_unified_address: None,
                 output_index: 0,
                 account_id: AccountId::ZERO,
                 scope: summary::Scope::from(zip32::Scope::Internal),
             }])
            .outgoing_sapling_notes(vec![OutgoingNoteSummary {
                 output_index: 0,
                 value: first_send_to_sapling,
                 memo: None,
                 recipient: "zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p".to_string(),
                 recipient_unified_address: None,
                 account_id: AccountId::ZERO,
                 scope: summary::Scope::from(zip32::Scope::External),
             }])
            .build()
            .unwrap();

        // Send to faucet (external) transparent
        let first_send_to_transparent = 20_000;
        let summary_external_transparent = TransactionSummaryBuilder::new()
            .blockheight(BlockHeight::from_u32(7))
            // We're not monitoring the mempool for this test
            .status(ConfirmationStatus::Transmitted(BlockHeight::from_u32(7)))
            .datetime(0)
            .txid(utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap())
            .value(first_send_to_transparent)
            .zec_price(None)
            .kind(TransactionKind::Sent(SendType::Send))
            .fee(Some(15_000))
            .orchard_notes(vec![BasicNoteSummary::from_parts(
                99_925_000,
                SpendStatus::Unspent,
                0,
                None,
            )])
            .sapling_notes(vec![])
            .transparent_coins(vec![])
            .outgoing_orchard_notes(vec![OutgoingNoteSummary {
                 output_index: 0,
                 value: 99_925_000,
                 memo: None,
                 recipient: "uregtest1ue949txhf9t2z6ldg8wc6s5t439t2hu55yh9l58gc23cmxthths836nxtpyvhpkrftsp2jnnp9eadtqy2nefxn04eyxeu8l0x5kk8ct9".to_string(),
                 recipient_unified_address: None,
                 account_id: AccountId::ZERO,
                 scope: summary::Scope::from(zip32::Scope::Internal),
             }])
            .outgoing_sapling_notes(vec![])
            .build()
            .unwrap();

        from_inputs::quick_send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "transparent"),
                first_send_to_transparent,
                None,
            )],
        )
        .await
        .unwrap();

        // Assert transactions are as expected
        assert_transaction_summary_equality(
            &recipient.transaction_summaries().await.0[0],
            &summary_orchard_receipt,
        );
        assert_transaction_summary_equality(
            &recipient.transaction_summaries().await.0[1],
            &summary_external_sapling,
        );
        assert_transaction_summary_equality(
            &recipient.transaction_summaries().await.0[2],
            &summary_external_transparent,
        );

        // Check several expectations about recipient wallet state:
        //  (1) shielded balance total is expected amount
        let expected_funds = recipient_initial_funds
            - first_send_to_sapling
            - (4 * u64::from(MARGINAL_FEE))
            - first_send_to_transparent
            - (3 * u64::from(MARGINAL_FEE));

        {
            let recipient_wallet = recipient.wallet.lock().await;
            assert_eq!(
                recipient_wallet.pending_balance::<OrchardNote>().await,
                Some(expected_funds)
            );
            //  (2) The balance is not yet verified
            assert_eq!(
                recipient_wallet.confirmed_balance::<OrchardNote>().await,
                Some(0)
            );
        }

        zingolib::testutils::increase_height_and_wait_for_client(regtest_manager, &mut faucet, 1)
            .await
            .unwrap();

        let recipient_second_funding = 1_000_000;
        let summary_orchard_receipt_2 = TransactionSummaryBuilder::new()
            .blockheight(BlockHeight::from_u32(8))
            .status(ConfirmationStatus::Confirmed(BlockHeight::from_u32(8)))
            .datetime(0)
            .txid(utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap())
            .value(recipient_second_funding)
            .zec_price(None)
            .kind(TransactionKind::Received)
            .fee(Some(10_000))
            .orchard_notes(vec![BasicNoteSummary::from_parts(
                recipient_second_funding,
                SpendStatus::Spent(
                    utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
                ),
                0,
                Some("Second wave incoming".to_string()),
            )])
            .sapling_notes(vec![])
            .transparent_coins(vec![])
            .outgoing_orchard_notes(vec![])
            .outgoing_sapling_notes(vec![])
            .build()
            .unwrap();
        from_inputs::quick_send(
            &faucet,
            vec![(
                &get_base_address_macro!(recipient, "unified"),
                recipient_second_funding,
                Some("Second wave incoming"),
            )],
        )
        .await
        .unwrap();
        zingolib::testutils::increase_height_and_wait_for_client(
            regtest_manager,
            &mut recipient,
            1,
        )
        .await
        .unwrap();

        // Send to external (faucet) transparent
        let second_send_to_transparent = 20_000;
        let summary_exteranl_transparent_2 = TransactionSummaryBuilder::new()
            .blockheight(BlockHeight::from_u32(9))
            .status(ConfirmationStatus::Confirmed(BlockHeight::from_u32(9)))
            .datetime(0)
            .txid(utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap())
            .value(second_send_to_transparent)
            .zec_price(None)
            .kind(TransactionKind::Sent(SendType::Send))
            .fee(Some(15_000))
            .orchard_notes(vec![BasicNoteSummary::from_parts(
                965_000,
                SpendStatus::Spent(
                    utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
                ),
                0,
                None,
            )])
            .sapling_notes(vec![])
            .transparent_coins(vec![])
            .outgoing_orchard_notes(vec![OutgoingNoteSummary {
                 output_index: 0,
                 value: 965_000,
                 memo: None,
                 recipient: "uregtest1ue949txhf9t2z6ldg8wc6s5t439t2hu55yh9l58gc23cmxthths836nxtpyvhpkrftsp2jnnp9eadtqy2nefxn04eyxeu8l0x5kk8ct9".to_string(),
                 recipient_unified_address: None,
                 account_id: AccountId::ZERO,
                 scope: summary::Scope::from(zip32::Scope::Internal),
             }])
            .outgoing_sapling_notes(vec![])
            .build()
            .unwrap();
        from_inputs::quick_send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "transparent"),
                second_send_to_transparent,
                None,
            )],
        )
        .await
        .unwrap();

        // Send to faucet (external) sapling 2
        let second_send_to_sapling = 20_000;
        let summary_external_sapling_2 = TransactionSummaryBuilder::new()
            .blockheight(BlockHeight::from_u32(9))
            .status(ConfirmationStatus::Confirmed(BlockHeight::from_u32(9)))
            .datetime(0)
            .txid(utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap())
            .value(second_send_to_sapling)
            .zec_price(None)
            .kind(TransactionKind::Sent(SendType::Send))
            .fee(Some(20_000))
            .orchard_notes(vec![BasicNoteSummary::from_parts(
                99_885_000,
                SpendStatus::Unspent,
                0,
                None,
            )])
            .sapling_notes(vec![])
            .transparent_coins(vec![])
            .outgoing_orchard_notes(vec![OutgoingNoteSummary {
                 output_index: 0,
                 value: 99_885_000,
                 memo: None,
                 recipient: "uregtest1ue949txhf9t2z6ldg8wc6s5t439t2hu55yh9l58gc23cmxthths836nxtpyvhpkrftsp2jnnp9eadtqy2nefxn04eyxeu8l0x5kk8ct9".to_string(),
                 recipient_unified_address: None,
                 account_id: AccountId::ZERO,
                 scope: summary::Scope::from(zip32::Scope::Internal),
             }])
            .outgoing_sapling_notes(vec![OutgoingNoteSummary {
                output_index: 0,
                 value: second_send_to_sapling,
                memo: None,
                recipient: "zregtestsapling1fmq2ufux3gm0v8qf7x585wj56le4wjfsqsj27zprjghntrerntggg507hxh2ydcdkn7sx8kya7p".to_string(),
                recipient_unified_address: None,
                 account_id: AccountId::ZERO,
                 scope: summary::Scope::from(zip32::Scope::External),
            }])
            .build()
            .unwrap();
        from_inputs::quick_send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "sapling"),
                second_send_to_sapling,
                None,
            )],
        )
        .await
        .unwrap();
        zingolib::testutils::increase_height_and_wait_for_client(
            regtest_manager,
            &mut recipient,
            1,
        )
        .await
        .unwrap();

        // Third external transparent
        let external_transparent_3 = 20_000;
        let summary_external_transparent_3 = TransactionSummaryBuilder::new()
            .blockheight(BlockHeight::from_u32(10))
            .status(ConfirmationStatus::Confirmed(BlockHeight::from_u32(10)))
            .datetime(0)
            .txid(utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap())
            .value(external_transparent_3)
            .zec_price(None)
            .kind(TransactionKind::Sent(SendType::Send))
            .fee(Some(15_000))
            .orchard_notes(vec![BasicNoteSummary::from_parts(
                930_000,
                SpendStatus::Unspent,
                0,
                None,
            )])
            .sapling_notes(vec![])
            .transparent_coins(vec![])
            .outgoing_orchard_notes(vec![OutgoingNoteSummary {
                 output_index: 0,
                 value: 930_000,
                 memo: None,
                 recipient: "uregtest1ue949txhf9t2z6ldg8wc6s5t439t2hu55yh9l58gc23cmxthths836nxtpyvhpkrftsp2jnnp9eadtqy2nefxn04eyxeu8l0x5kk8ct9".to_string(),
                 recipient_unified_address: None,
                 account_id: AccountId::ZERO,
                 scope: summary::Scope::from(zip32::Scope::Internal),
             }])
            .outgoing_sapling_notes(vec![])
            .build()
            .unwrap();
        from_inputs::quick_send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "transparent"),
                external_transparent_3,
                None,
            )],
        )
        .await
        .unwrap();
        zingolib::testutils::increase_height_and_wait_for_client(
            regtest_manager,
            &mut recipient,
            1,
        )
        .await
        .unwrap();

        // Final check
        assert_transaction_summary_equality(
            &recipient.transaction_summaries().await.0[3],
            &summary_orchard_receipt_2,
        );
        assert_transaction_summary_exists(&recipient, &summary_exteranl_transparent_2).await; // due to summaries of the same blockheight changing order
        assert_transaction_summary_exists(&recipient, &summary_external_sapling_2).await; // we check all summaries for these expected transactions
        assert_transaction_summary_equality(
            &recipient.transaction_summaries().await.0[6],
            &summary_external_transparent_3,
        );
        let second_wave_expected_funds = expected_funds + recipient_second_funding
            - second_send_to_sapling
            - second_send_to_transparent
            - external_transparent_3
            - (5 * u64::from(MINIMUM_FEE));
        assert_eq!(
            recipient
                .wallet
                .lock()
                .await
                .spendable_balance::<OrchardNote>()
                .await,
            Some(second_wave_expected_funds),
        );
    }

    #[tokio::test]
    async fn send_orchard_back_and_forth() {
        // setup
        let (regtest_manager, _cph, mut faucet, mut recipient) =
            scenarios::faucet_recipient_default().await;
        let faucet_to_recipient_amount = 20_000u64;
        let recipient_to_faucet_amount = 5_000u64;
        // check start state
        faucet.sync_and_await(true).await.unwrap();
        let wallet_fully_scanned_height = faucet
            .wallet
            .lock()
            .await
            .sync_state
            .fully_scanned_height()
            .unwrap();
        assert_eq!(wallet_fully_scanned_height, BASE_HEIGHT.into());
        let three_blocks_reward = block_rewards::CANOPY
            .checked_mul(BASE_HEIGHT as u64)
            .unwrap();
        check_client_balances!(faucet, o: three_blocks_reward s: 0 t: 0);

        // post transfer to recipient, and verify
        from_inputs::quick_send(
            &faucet,
            vec![(
                &get_base_address_macro!(recipient, "unified"),
                faucet_to_recipient_amount,
                Some("Orcharding"),
            )],
        )
        .await
        .unwrap();
        let orch_change =
            block_rewards::CANOPY - (faucet_to_recipient_amount + u64::from(MINIMUM_FEE));
        zingolib::testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &mut recipient,
            1,
        )
        .await
        .unwrap();
        faucet.sync_and_await(true).await.unwrap();
        let faucet_orch = three_blocks_reward + orch_change + u64::from(MINIMUM_FEE);

        println!(
            "{}",
            JsonValue::from(faucet.sorted_value_transfers(true).await).pretty(4)
        );
        println!(
            "{}",
            serde_json::to_string_pretty(&faucet.do_balance().await).unwrap()
        );

        check_client_balances!(faucet, o: faucet_orch s: 0 t: 0);
        check_client_balances!(recipient, o: faucet_to_recipient_amount s: 0 t: 0);

        // post half back to faucet, and verify
        from_inputs::quick_send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "unified"),
                recipient_to_faucet_amount,
                Some("Sending back"),
            )],
        )
        .await
        .unwrap();
        zingolib::testutils::increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 1)
            .await
            .unwrap();
        recipient.sync_and_await(true).await.unwrap();

        let faucet_final_orch = faucet_orch
            + recipient_to_faucet_amount
            + block_rewards::CANOPY
            + u64::from(MINIMUM_FEE);
        let recipient_final_orch =
            faucet_to_recipient_amount - (u64::from(MINIMUM_FEE) + recipient_to_faucet_amount);
        check_client_balances!(
            faucet,
            o: faucet_final_orch s: 0 t: 0
        );
        check_client_balances!(recipient, o: recipient_final_orch s: 0 t: 0);
    }
    #[tokio::test]
    async fn send_mined_sapling_to_orchard() {
        // This test shows a confirmation changing the state of balance by
        // debiting unverified_orchard_balance and crediting verified_orchard_balance.  The debit amount is
        // consistent with all the notes in the relevant block changing state.
        // NOTE that the balance doesn't give insight into the distribution across notes.
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let (regtest_manager, _cph, mut faucet) = scenarios::faucet(
            PoolType::Shielded(ShieldedProtocol::Sapling),
            regtest_network,
            true,
        )
        .await;

        {
            let wallet = faucet.wallet.lock().await;
            dbg!(wallet.sync_state.wallet_height());
            dbg!(wallet
                .shard_trees
                .sapling
                .store()
                .max_checkpoint_id()
                .unwrap());
            dbg!(wallet
                .shard_trees
                .orchard
                .store()
                .max_checkpoint_id()
                .unwrap());
            dbg!(wallet
                .shard_trees
                .sapling
                .root_at_checkpoint_id(&3.into())
                .unwrap());
            dbg!(wallet
                .shard_trees
                .orchard
                .root_at_checkpoint_id(&3.into())
                .unwrap());
        }

        let amount_to_send = 5_000;
        from_inputs::quick_send(
            &faucet,
            vec![(
                get_base_address_macro!(faucet, "unified").as_str(),
                amount_to_send,
                Some("Scenario test: engage!"),
            )],
        )
        .await
        .unwrap();
        zingolib::testutils::increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 1)
            .await
            .unwrap();
        let balance = faucet.do_balance().await;
        // We send change to orchard now, so we should have the full value of the note
        // we spent, minus the transaction fee
        assert_eq!(balance.unverified_orchard_balance, Some(0));
        assert_eq!(
            balance.verified_orchard_balance.unwrap(),
            625_000_000 - 4 * u64::from(MARGINAL_FEE)
        );
    }
    #[tokio::test]
    async fn send_heartwood_sapling_funds() {
        let regtest_network = RegtestNetwork::new(1, 1, 1, 1, 3, 5, 5);
        let (regtest_manager, _cph, mut faucet, mut recipient) = scenarios::faucet_recipient(
            PoolType::Shielded(ShieldedProtocol::Sapling),
            regtest_network,
            true,
        )
        .await;
        increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 3)
            .await
            .unwrap();
        check_client_balances!(faucet, o: 0 s: 3_500_000_000u64 t: 0);
        from_inputs::quick_send(
            &faucet,
            vec![(
                &get_base_address_macro!(recipient, "unified"),
                3_499_960_000u64,
                None,
            )],
        )
        .await
        .unwrap();
        check_client_balances!(faucet, o: 0 s: 0 t: 0);
        increase_height_and_wait_for_client(&regtest_manager, &mut recipient, 1)
            .await
            .unwrap();
        check_client_balances!(recipient, o: 3_499_960_000u64 s: 0 t: 0);
    }
    #[tokio::test]
    async fn send_funds_to_all_pools() {
        let regtest_network = RegtestNetwork::all_upgrades_active();
        let (
            _regtest_manager,
            _cph,
            _faucet,
            recipient,
            _orchard_txid,
            _sapling_txid,
            _transparent_txid,
        ) = scenarios::faucet_funded_recipient(
            Some(100_000),
            Some(100_000),
            Some(100_000),
            PoolType::Shielded(ShieldedProtocol::Orchard),
            regtest_network,
            true,
        )
        .await;
        check_client_balances!(recipient, o: 100_000 s: 100_000 t: 100_000);
    }
    #[tokio::test]
    async fn self_send_to_t_displays_as_one_transaction() {
        let (regtest_manager, _cph, mut faucet, mut recipient) =
            scenarios::faucet_recipient_default().await;
        let recipient_unified_address = get_base_address_macro!(recipient, "unified");
        let sent_value = 80_000;
        from_inputs::quick_send(
            &faucet,
            vec![(recipient_unified_address.as_str(), sent_value, None)],
        )
        .await
        .unwrap();
        zingolib::testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &mut recipient,
            1,
        )
        .await
        .unwrap();
        let recipient_taddr = get_base_address_macro!(recipient, "transparent");
        let recipient_zaddr = get_base_address_macro!(recipient, "sapling");
        let sent_to_taddr_value = 5_000;
        let sent_to_zaddr_value = 11_000;
        let sent_to_self_orchard_value = 1_000;
        from_inputs::quick_send(
            &recipient,
            vec![(recipient_taddr.as_str(), sent_to_taddr_value, None)],
        )
        .await
        .unwrap();
        zingolib::testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &mut recipient,
            1,
        )
        .await
        .unwrap();
        from_inputs::quick_send(
            &recipient,
            vec![
                (recipient_taddr.as_str(), sent_to_taddr_value, None),
                (recipient_zaddr.as_str(), sent_to_zaddr_value, Some("foo")),
                (
                    recipient_unified_address.as_str(),
                    sent_to_self_orchard_value,
                    Some("bar"),
                ),
            ],
        )
        .await
        .unwrap();
        faucet.sync_and_await(true).await.unwrap();
        from_inputs::quick_send(
            &faucet,
            vec![
                (recipient_taddr.as_str(), sent_to_taddr_value, None),
                (recipient_zaddr.as_str(), sent_to_zaddr_value, Some("foo2")),
                (
                    recipient_unified_address.as_str(),
                    sent_to_self_orchard_value,
                    Some("bar2"),
                ),
            ],
        )
        .await
        .unwrap();
        zingolib::testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &mut recipient,
            1,
        )
        .await
        .unwrap();
        println!(
            "{}",
            json::stringify_pretty(recipient.transaction_summaries().await, 4)
        );
        let mut txids = recipient.transaction_summaries().await.txids().into_iter();
        assert!(itertools::Itertools::all_unique(&mut txids));
    }

    #[tokio::test]
    async fn sapling_to_sapling_scan_together() {
        let funding_value = 100_000;
        let (regtest_manager, _cph, faucet, mut recipient, _, funding_txid, _) =
            scenarios::faucet_funded_recipient(
                None,
                Some(funding_value),
                None,
                PoolType::Shielded(ShieldedProtocol::Orchard),
                RegtestNetwork::all_upgrades_active(),
                false,
            )
            .await;

        let spent_value = 20_000;
        let faucet_sapling_address = get_base_address_macro!(faucet, "sapling");
        let spent_txid = from_inputs::quick_send(
            &recipient,
            vec![(&faucet_sapling_address, spent_value, None)],
        )
        .await
        .unwrap()
        .first()
        .to_string();

        zingolib::testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &mut recipient,
            1,
        )
        .await
        .unwrap();

        let transactions = recipient
            .wallet
            .lock()
            .await
            .transaction_summaries()
            .await
            .0;

        assert_eq!(transactions.first().unwrap().blockheight(), 5.into());
        assert_eq!(
            transactions.first().unwrap().txid().to_string(),
            funding_txid.unwrap()
        );
        assert_eq!(transactions.first().unwrap().value(), funding_value);

        assert_eq!(transactions.get(1).unwrap().blockheight(), 6.into());
        assert_eq!(transactions.get(1).unwrap().txid().to_string(), spent_txid);
        assert_eq!(transactions.get(1).unwrap().value(), spent_value);
        assert!(transactions
            .get(1)
            .unwrap()
            .outgoing_sapling_notes()
            .iter()
            .find(|note| { note.recipient == faucet_sapling_address })
            .is_some());
        assert!(transactions
            .get(1)
            .unwrap()
            .outgoing_sapling_notes()
            .iter()
            .find(|note| { note.value == spent_value })
            .is_some());
    }

    #[tokio::test]
    async fn sapling_incoming_sapling_outgoing() {
        // TODO:  Add assertions about Sapling change note.
        let (regtest_manager, _cph, faucet, mut recipient) =
            scenarios::faucet_recipient_default().await;
        let value = 100_000;

        // 2. Send an incoming transaction to fill the wallet
        let faucet_funding_txid = from_inputs::quick_send(
            &faucet,
            vec![(&get_base_address_macro!(&recipient, "sapling"), value, None)],
        )
        .await
        .unwrap()
        .first()
        .to_string();
        zingolib::testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &mut recipient,
            1,
        )
        .await
        .unwrap();

        assert_eq!(
            recipient
                .wallet
                .lock()
                .await
                .sync_state
                .fully_scanned_height()
                .unwrap(),
            4.into()
        );

        // 3. Check the balance is correct, and we received the incoming transaction from ?outside?
        let network = recipient.wallet.lock().await.network;
        let balance = recipient.do_balance().await;
        assert_eq!(balance.sapling_balance.unwrap(), value);
        assert_eq!(balance.unverified_sapling_balance.unwrap(), 0);
        assert_eq!(balance.spendable_sapling_balance.unwrap(), value);

        {
            let recipient_sapling_address = *recipient
                .wallet
                .lock()
                .await
                .unified_addresses
                .values()
                .next()
                .unwrap()
                .sapling()
                .unwrap();
            let transactions = &recipient.wallet.lock().await.wallet_transactions;
            assert_eq!(transactions.len(), 1);
            let received_transaction = transactions
                .get(&txid_from_hex_encoded_str(&faucet_funding_txid).unwrap())
                .unwrap();

            assert_eq!(received_transaction.txid().to_string(), faucet_funding_txid);
            assert_eq!(
                received_transaction
                    .status()
                    .get_confirmed_height()
                    .unwrap(),
                4.into()
            );

            let received_note = received_transaction
                .sapling_notes()
                .first()
                .unwrap()
                .clone();

            assert_eq!(received_note.value(), value);
            assert_eq!(received_note.note().recipient(), recipient_sapling_address);
        }

        // 4. Send z-to-z transaction to external z address with a memo
        let sent_value = 2_000;
        let outgoing_memo = "Outgoing Memo";

        let sent_transaction_id = from_inputs::quick_send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "sapling"),
                sent_value,
                Some(outgoing_memo),
            )],
        )
        .await
        .unwrap()
        .first()
        .to_string();

        // 5. Check the pending transaction is present
        // 5.1 Check notes

        let sapling_notes = recipient
            .wallet
            .lock()
            .await
            .note_summaries::<SaplingNote>(true);

        assert_eq!(
            recipient
                .wallet
                .lock()
                .await
                .wallet_outputs::<OrchardNote>()
                .len(),
            0
        );
        assert_eq!(
            sapling_notes
                .iter()
                .filter(|note| note.spend_status.is_confirmed_spent())
                .count(),
            0
        );
        assert_eq!(
            sapling_notes
                .iter()
                .filter(|note| note.spend_status.is_pending_spent())
                .count(),
            1
        );

        let pending_sapling_note = *sapling_notes
            .iter()
            .filter(|&note| note.spend_status.is_pending_spent())
            .collect::<Vec<_>>()
            .first()
            .unwrap();
        assert_eq!(pending_sapling_note.txid.to_string(), faucet_funding_txid);
        if let SpendStatus::TransmittedSpent(txid) = pending_sapling_note.spend_status {
            assert_eq!(txid.to_string(), sent_transaction_id);
        } else {
            panic!("incorrect spend status!");
        }

        {
            // Check transaction list
            let transactions = &recipient.wallet.lock().await.wallet_transactions;
            assert_eq!(transactions.len(), 2);
            let sent_transaction = transactions
                .get(&txid_from_hex_encoded_str(&sent_transaction_id).unwrap())
                .unwrap();

            assert_eq!(sent_transaction.txid().to_string(), sent_transaction_id);
            assert_eq!(sent_transaction.total_value_sent(), sent_value);
            assert!(!sent_transaction.status().is_confirmed());
            assert_eq!(sent_transaction.status().get_height(), 5.into());

            let faucet_sapling_address = get_base_address_macro!(faucet, "sapling");
            let outgoing_sapling_note = sent_transaction
                .outgoing_sapling_notes()
                .iter()
                .find(|note| note.encoded_recipient(&network) == faucet_sapling_address)
                .unwrap();
            if let Memo::Text(memo) = outgoing_sapling_note.memo() {
                assert_eq!(&String::from(memo.clone()), outgoing_memo);
            } else {
                panic!("no text memo");
            }
            assert_eq!(outgoing_sapling_note.value(), sent_value);
        }

        // 6. Mine the sent transaction
        zingolib::testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &mut recipient,
            1,
        )
        .await
        .unwrap();

        {
            let transactions = &recipient.wallet.lock().await.wallet_transactions;
            let sent_transaction = transactions
                .get(&txid_from_hex_encoded_str(&sent_transaction_id).unwrap())
                .unwrap();
            assert!(sent_transaction.status().is_confirmed());
            assert_eq!(
                sent_transaction.status().get_confirmed_height().unwrap(),
                5.into()
            );
        }

        // 7. Check the notes to see that we have one spent sapling note and one unspent sapling note (change)
        // Which is immediately spendable.
        let recipient_wallet = recipient.wallet.lock().await;
        let sapling_notes = recipient_wallet.note_summaries::<SaplingNote>(true);
        let orchard_notes = recipient_wallet.note_summaries::<OrchardNote>(true);

        assert!(orchard_notes.is_empty());
        assert_eq!(sapling_notes.len(), 2);
        assert_eq!(
            sapling_notes
                .iter()
                .filter(|&note| note.spend_status.is_unspent())
                .count(),
            1
        );
        assert_eq!(
            sapling_notes
                .iter()
                .filter(|&note| note.spend_status.is_confirmed_spent())
                .count(),
            1
        );
        let spent_sapling_note = sapling_notes
            .iter()
            .find(|&note| note.spend_status.is_confirmed_spent())
            .unwrap();
        assert_eq!(spent_sapling_note.block_height, 4.into());
        assert_eq!(spent_sapling_note.value, value);
        if let SpendStatus::Spent(txid) = spent_sapling_note.spend_status {
            assert_eq!(txid.to_string(), sent_transaction_id);
            assert_eq!(
                recipient_wallet
                    .wallet_transactions
                    .get(&txid)
                    .unwrap()
                    .status()
                    .get_confirmed_height()
                    .unwrap(),
                5.into()
            );
        } else {
            panic!("note not spent!")
        }
    }

    #[tokio::test]
    async fn sapling_dust_fee_collection() {
        let (regtest_manager, __cph, faucet, mut recipient) =
            scenarios::faucet_recipient_default().await;
        let recipient_sapling = get_base_address_macro!(recipient, "sapling");
        let recipient_unified = get_base_address_macro!(recipient, "unified");
        check_client_balances!(recipient, o: 0 s: 0 t: 0);
        let fee = u64::from(MINIMUM_FEE);
        let for_orchard = dbg!(fee * 10);
        let for_sapling = dbg!(fee / 10);
        from_inputs::quick_send(
            &faucet,
            vec![
                (&recipient_unified, for_orchard, Some("Plenty for orchard.")),
                (&recipient_sapling, for_sapling, Some("Dust for sapling.")),
            ],
        )
        .await
        .unwrap();
        zingolib::testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &mut recipient,
            1,
        )
        .await
        .unwrap();
        check_client_balances!(recipient, o: for_orchard s: for_sapling t: 0 );

        from_inputs::quick_send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "unified"),
                fee * 5,
                Some("Five times fee."),
            )],
        )
        .await
        .unwrap();
        zingolib::testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &mut recipient,
            1,
        )
        .await
        .unwrap();
        let remaining_orchard = for_orchard - (6 * fee);
        check_client_balances!(recipient, o: remaining_orchard s: for_sapling t: 0);
    }
    // FIXME: zingo2 yet to implement transaction filter in sync engine. its also not clear how this test exceeds the tx filter.
    // #[tokio::test]
    // async fn sandblast_filter_preserves_trees() {
    //     let (ref regtest_manager, _cph, ref faucet, ref recipient, _txid) =
    //         scenarios::faucet_funded_recipient_default(100_000).await;
    //     recipient
    //         .wallet
    //         .wallet_options
    //         .write()
    //         .await
    //         .transaction_size_filter = Some(10);
    //     recipient.do_sync(false).await.unwrap();
    //     dbg!(
    //         recipient
    //             .wallet
    //             .wallet_options
    //             .read()
    //             .await
    //             .transaction_size_filter
    //     );

    //     println!("creating vec");
    //     from_inputs::quick_send(
    //         faucet,
    //         vec![(&get_base_address_macro!(faucet, "unified"), 10, None); 15],
    //     )
    //     .await
    //     .unwrap();
    //     zingolib::testutils::increase_height_and_wait_for_client(regtest_manager, recipient, 10)
    //         .await
    //         .unwrap();
    //     from_inputs::quick_send(
    //         recipient,
    //         vec![(&get_base_address_macro!(faucet, "unified"), 10, None)],
    //     )
    //     .await
    //     .unwrap();
    //     zingolib::testutils::increase_height_and_wait_for_client(regtest_manager, recipient, 10)
    //         .await
    //         .unwrap();
    //     faucet.do_sync(false).await.unwrap();
    //     assert_eq!(
    //         faucet
    //             .wallet
    //             .transaction_context
    //             .transaction_metadata_set
    //             .read()
    //             .await
    //             .witness_trees()
    //             .unwrap()
    //             .witness_tree_orchard
    //             .max_leaf_position(None),
    //         recipient
    //             .wallet
    //             .transaction_context
    //             .transaction_metadata_set
    //             .read()
    //             .await
    //             .witness_trees()
    //             .unwrap()
    //             .witness_tree_orchard
    //             .max_leaf_position(None)
    //     );
    // }
    /// This mod collects tests of outgoing_metadata (a TransactionRecordField) across rescans
    mod rescan_still_have_outgoing_notes {
        use super::*;

        #[tokio::test]
        async fn self_send() {
            let (regtest_manager, _cph, mut faucet) = scenarios::faucet_default().await;
            let faucet_sapling_addr = get_base_address_macro!(faucet, "sapling");
            let mut txids = vec![];
            for memo in [None, Some("Second Transaction")] {
                txids.push(
                    *from_inputs::quick_send(
                        &faucet,
                        vec![(faucet_sapling_addr.as_str(), 100_000, memo)],
                    )
                    .await
                    .unwrap()
                    .first(),
                );
                zingolib::testutils::increase_height_and_wait_for_client(
                    &regtest_manager,
                    &mut faucet,
                    1,
                )
                .await
                .unwrap();
            }

            let pre_rescan_summaries = faucet.transaction_summaries().await;
            faucet.rescan_and_await(true).await.unwrap();
            let post_rescan_summaries = faucet.transaction_summaries().await;
            assert_eq!(pre_rescan_summaries, post_rescan_summaries);
        }
        #[tokio::test]
        async fn external_send() {
            let (regtest_manager, _cph, mut faucet, recipient) =
                scenarios::faucet_recipient_default().await;
            let _external_send_txid_with_memo = *from_inputs::quick_send(
                &faucet,
                vec![(
                    get_base_address_macro!(recipient, "sapling").as_str(),
                    1_000,
                    Some("foo"),
                )],
            )
            .await
            .unwrap()
            .first();
            let _external_send_txid_no_memo = *from_inputs::quick_send(
                &faucet,
                vec![(
                    get_base_address_macro!(recipient, "sapling").as_str(),
                    1_000,
                    None,
                )],
            )
            .await
            .unwrap()
            .first();
            // TODO:  This chain height bump should be unnecessary. I think removing
            // this increase_height call reveals a bug!
            zingolib::testutils::increase_height_and_wait_for_client(
                &regtest_manager,
                &mut faucet,
                1,
            )
            .await
            .unwrap();

            let pre_rescan_summaries = faucet.transaction_summaries().await;
            faucet.rescan_and_await(true).await.unwrap();
            let post_rescan_summaries = faucet.transaction_summaries().await;
            assert_eq!(pre_rescan_summaries, post_rescan_summaries);
        }
        #[tokio::test]
        async fn check_list_value_transfers_across_rescan() {
            let inital_value = 100_000;
            let (ref regtest_manager, _cph, faucet, mut recipient, _txid) =
                scenarios::faucet_funded_recipient_default(inital_value).await;
            from_inputs::quick_send(
                &recipient,
                vec![(&get_base_address_macro!(faucet, "unified"), 10_000, None); 2],
            )
            .await
            .unwrap();
            zingolib::testutils::increase_height_and_wait_for_client(
                regtest_manager,
                &mut recipient,
                1,
            )
            .await
            .unwrap();
            let pre_rescan_transactions = recipient.transaction_summaries().await;
            let pre_rescan_summaries = recipient.sorted_value_transfers(true).await;
            recipient.rescan_and_await(true).await.unwrap();
            let post_rescan_transactions = recipient.transaction_summaries().await;
            let post_rescan_summaries = recipient.sorted_value_transfers(true).await;
            assert_eq!(pre_rescan_transactions, post_rescan_transactions);
            assert_eq!(pre_rescan_summaries, post_rescan_summaries);
        }
    }
    #[tokio::test]
    async fn note_selection_order() {
        // In order to fund a transaction multiple notes may be selected and consumed.
        // The algorithm selects the smallest covering note(s).
        // In addition to testing the order in which notes are selected this test:
        //   * sends to a sapling address
        //   * sends back to the original sender's UA
        let (regtest_manager, _cph, mut faucet, mut recipient) =
            scenarios::faucet_recipient_default().await;
        zingolib::testutils::increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 5)
            .await
            .unwrap();

        let client_2_saplingaddress = get_base_address_macro!(recipient, "sapling");
        // Send three transfers in increasing 10_000 zat increments
        // These are sent from the coinbase funded client which will
        // subsequently receive funding via it's orchard-packed UA.
        let memos = ["1", "2", "3"];
        from_inputs::quick_send(
            &faucet,
            (1..=3)
                .map(|n| {
                    (
                        client_2_saplingaddress.as_str(),
                        n * 10_000,
                        Some(memos[(n - 1) as usize]),
                    )
                })
                .collect(),
        )
        .await
        .unwrap();

        zingolib::testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &mut recipient,
            5,
        )
        .await
        .unwrap();
        // We know that the largest single note that 2 received from 1 was 30_000, for 2 to send
        // 30_000 back to 1 it will have to collect funds from two notes to pay the full 30_000
        // plus the transaction fee.
        from_inputs::quick_send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "unified"),
                30_000,
                Some("Sending back, should have 2 inputs"),
            )],
        )
        .await
        .unwrap();

        // FIXME: this test has all its assertions commented out !?
        /*
        let client_2_notes = recipient.do_list_notes(false).await;
        // The 30_000 zat note to cover the value, plus another for the tx-fee.
        let first_value = client_2_notes["pending_sapling_notes"][0]["value"]
            .as_fixed_point_u64(0)
            .unwrap();
        let second_value = client_2_notes["pending_sapling_notes"][1]["value"]
            .as_fixed_point_u64(0)
            .unwrap();
        assert!(
            first_value == 30_000u64 && second_value == 20_000u64
                || first_value == 20_000u64 && second_value == 30_000u64
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
        assert_eq!(change_note["value"], 20000 - u64::from(MINIMUM_FEE));
        let non_change_note_values = client_2_notes["unspent_sapling_notes"]
            .members()
            .filter(|note| !note["is_change"].as_bool().unwrap())
            .map(extract_value_as_u64)
            .collect::<Vec<u64>>();
        */
        // client_2 got a total of 3000+2000+1000
        // It sent 3000 to the client_1, and also
        // paid the default transaction fee.
        // In non change notes it has 1000.
        // There is an outstanding 2000 that is marked as change.
        // After sync the unspent_sapling_notes should go to 3000.
        /*
        assert_eq!(non_change_note_values.iter().sum::<u64>(), 10000u64);

        zingolib::testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 5)
            .await
            .unwrap();
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
                .chain(client_2_post_transaction_notes["unspent_orchard_notes"].members())
                .map(extract_value_as_u64)
                .sum::<u64>(),
            20000u64 // 10000 received and unused + (20000 - 10000 txfee)
        );

        // More explicit than ignoring the unused variable, we only care about this in order to drop it
        */
    }

    // FIXME: it seems this test makes assertions on mempool but mempool monitoring is off?
    #[tokio::test]
    async fn mempool_and_balance() {
        let value = 100_000;
        let (regtest_manager, _cph, faucet, mut recipient, _txid) =
            scenarios::faucet_funded_recipient_default(value).await;

        let bal = recipient.do_balance().await;
        println!("{}", serde_json::to_string_pretty(&bal).unwrap());
        assert_eq!(bal.orchard_balance.unwrap(), value);
        assert_eq!(bal.unverified_orchard_balance.unwrap(), 0);
        assert_eq!(bal.verified_orchard_balance.unwrap(), value);

        // 3. Mine 10 blocks
        zingolib::testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &mut recipient,
            10,
        )
        .await
        .unwrap();
        let bal = recipient.do_balance().await;
        assert_eq!(bal.orchard_balance.unwrap(), value);
        assert_eq!(bal.verified_orchard_balance.unwrap(), value);
        assert_eq!(bal.unverified_orchard_balance.unwrap(), 0);

        // 4. Spend the funds
        let sent_value = 2000;
        let outgoing_memo = "Outgoing Memo";

        let _sent_transaction_id = from_inputs::quick_send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "unified"),
                sent_value,
                Some(outgoing_memo),
            )],
        )
        .await
        .unwrap();

        let bal = recipient.do_balance().await;

        // Even though the transaction is not mined (in the mempool) the balances should be updated to reflect the spent funds
        let new_bal = value - (sent_value + u64::from(MINIMUM_FEE));
        assert_eq!(bal.orchard_balance.unwrap(), new_bal);
        assert_eq!(bal.verified_orchard_balance.unwrap(), 0);
        assert_eq!(bal.unverified_orchard_balance.unwrap(), new_bal);

        // 5. Mine the pending block, making the funds verified and spendable.
        zingolib::testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &mut recipient,
            10,
        )
        .await
        .unwrap();

        let bal = recipient.do_balance().await;

        assert_eq!(bal.orchard_balance.unwrap(), new_bal);
        assert_eq!(bal.verified_orchard_balance.unwrap(), new_bal);
        assert_eq!(bal.unverified_orchard_balance.unwrap(), 0);
    }

    // FIXME: add unified address regeneration to pepper sync and add a test here

    #[tokio::test]
    async fn list_value_transfers_check_fees() {
        // Check that list_value_transfers behaves correctly given different fee scenarios
        let (regtest_manager, _cph, mut client_builder, regtest_network) =
            scenarios::custom_clients_default().await;
        let mut faucet = client_builder.build_faucet(false, regtest_network);
        let mut pool_migration_client = client_builder.build_client(
            HOSPITAL_MUSEUM_SEED.to_string(),
            0,
            false,
            regtest_network,
        );
        let pmc_taddr = get_base_address_macro!(pool_migration_client, "transparent");
        let pmc_sapling = get_base_address_macro!(pool_migration_client, "sapling");
        let pmc_unified = get_base_address_macro!(pool_migration_client, "unified");
        // Ensure that the client has confirmed spendable funds
        zingolib::testutils::increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 3)
            .await
            .unwrap();
        macro_rules! bump_and_check_pmc {
            (o: $o:tt s: $s:tt t: $t:tt) => {
                zingolib::testutils::increase_height_and_wait_for_client(&regtest_manager, &mut pool_migration_client, 1).await.unwrap();
                check_client_balances!(pool_migration_client, o:$o s:$s t:$t);
            };
        }

        // pmc receives 100_000 orchard
        from_inputs::quick_send(&faucet, vec![(&pmc_unified, 100_000, None)])
            .await
            .unwrap();
        bump_and_check_pmc!(o: 100_000 s: 0 t: 0);

        // to transparent and sapling from orchard
        //
        // Expected Fees:
        // 5_000 for transparent + 10_000 for orchard + 10_000 for sapling == 25_000
        from_inputs::quick_send(
            &pool_migration_client,
            vec![(&pmc_taddr, 30_000, None), (&pmc_sapling, 30_000, None)],
        )
        .await
        .unwrap();
        bump_and_check_pmc!(o: 15_000 s: 30_000 t: 30_000);
    }

    #[tokio::test]
    async fn from_t_z_o_tz_to_zo_tzo_to_orchard() {
        // Test all possible promoting note source combinations
        let (regtest_manager, _cph, mut client_builder, regtest_network) =
            scenarios::custom_clients_default().await;
        let mut faucet = client_builder.build_faucet(false, regtest_network);
        let mut client = client_builder.build_client(
            HOSPITAL_MUSEUM_SEED.to_string(),
            0,
            false,
            regtest_network,
        );
        let pmc_taddr = get_base_address_macro!(client, "transparent");
        let pmc_sapling = get_base_address_macro!(client, "sapling");
        let pmc_unified = get_base_address_macro!(client, "unified");

        // Ensure that the faucet has confirmed spendable funds
        zingolib::testutils::increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 1)
            .await
            .unwrap();

        macro_rules! bump_and_check {
            (o: $o:tt s: $s:tt t: $t:tt) => {
                zingolib::testutils::increase_height_and_wait_for_client(&regtest_manager, &mut client, 1).await.unwrap();
                check_client_balances!(client, o:$o s:$s t:$t);
            };
        }

        let mut test_dev_total_expected_fee = 0;
        // 1 pmc receives 50_000 transparent
        //  # Expected Fees to recipient:
        //    - legacy: 0
        //    - 317:    0
        from_inputs::quick_send(&faucet, vec![(&pmc_taddr, 50_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 0 s: 0 t: 50_000);
        assert_eq!(test_dev_total_expected_fee, 0);

        // 2 pmc shields 50_000 transparent, to orchard paying fee
        //  t -> o
        //  # Expected Fees to recipient:
        //    - legacy: 10_000
        //    - 317:    15_000 1-orchard + 1-dummy + 1-transparent in
        client.quick_shield().await.unwrap();
        bump_and_check!(o: 35_000 s: 0 t: 0);
        test_dev_total_expected_fee += 15_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 3 pmc receives 50_000 sapling
        //  # Expected Fees to recipient:
        //    - legacy: 0
        //    - 317:    0
        from_inputs::quick_send(&faucet, vec![(&pmc_sapling, 50_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 35_000 s: 50_000 t: 0);
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 4 pmc migrates 40_000 from sapling to orchard plus fee
        //  z -> o
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    20_000
        from_inputs::quick_send(&client, vec![(&pmc_unified, 30_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 65_000 s: 0 t: 0);
        test_dev_total_expected_fee += 20_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 5 Self send of 55_000 paying 10_000 fee
        //  o -> o
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    10_000
        from_inputs::quick_send(&client, vec![(&pmc_unified, 55_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 55_000 s: 0 t: 0);
        test_dev_total_expected_fee += 10_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 6 to transparent and sapling from orchard
        //  o -> tz
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    5_000 for transparent out + 10_000 for orchard + 10_000 for sapling == 25_000
        from_inputs::quick_send(
            &client,
            vec![(&pmc_taddr, 10_000, None), (&pmc_sapling, 10_000, None)],
        )
        .await
        .unwrap();
        bump_and_check!(o: 10_000 s: 10_000 t: 10_000);
        test_dev_total_expected_fee += 25_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 7 Receive 500_000 to transparent
        from_inputs::quick_send(&faucet, vec![(&pmc_taddr, 500_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 10_000 s: 10_000 t: 510_000);
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 8 Shield transparent to orchard
        //  t -> o
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    20_000 = 10_000 orchard and o-dummy + 10_000 (2 t-notes)
        client.quick_shield().await.unwrap();
        bump_and_check!(o: 500_000 s: 10_000 t: 0);
        test_dev_total_expected_fee += 20_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 9 self o send orchard to orchard
        // TODO: already tested!?
        //  o -> o
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    10_000
        from_inputs::quick_send(&client, vec![(&pmc_unified, 30_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 490_000 s: 10_000 t: 0);
        test_dev_total_expected_fee += 10_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        println!(
            "{}",
            client
                .wallet
                .lock()
                .await
                .note_summaries::<SaplingNote>(false)
        );
        println!(
            "{}",
            client
                .wallet
                .lock()
                .await
                .note_summaries::<OrchardNote>(false)
        );

        // 10 Orchard and Sapling demote all to transparent self-send
        //  oz -> t
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    15_000 5-o (3 dust)- 10_000 orchard, 1 utxo 5_000 transparent
        from_inputs::quick_send(&client, vec![(&pmc_taddr, 475_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 0 s: 0 t: 475_000);
        test_dev_total_expected_fee += 25_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 10 transparent to transparent
        // Very explicit catch of reject sending from transparent to other than Self Orchard
        match from_inputs::quick_send(&client, vec![(&pmc_taddr, 1, None)]).await {
            Ok(_) => panic!(),
            Err(QuickSendError::ProposeSend(proposesenderror)) => match proposesenderror {
                ProposeSendError::Proposal(insufficient) => match insufficient {
                    zcash_client_backend::data_api::error::Error::InsufficientFunds {
                        available,
                        required,
                    } => {
                        assert_eq!(available, NonNegativeAmount::from_u64(20_000).unwrap());
                        assert_eq!(required, NonNegativeAmount::from_u64(25_001).unwrap());
                    }
                    _ => panic!(),
                },
                ProposeSendError::TransactionRequestFailed(_) => panic!(),
                ProposeSendError::ZeroValueSendAll => panic!(),
                ProposeSendError::BalanceError(_) => panic!(),
            },
            _ => panic!(),
        }
        bump_and_check!(o: 10_000 s: 10_000 t: 465_000);
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 11 transparent to sapling
        //  t -> z
        // 10 transparent to transparent
        // Very explicit catch of reject sending from transparent to other than Self Orchard
        match from_inputs::quick_send(&client, vec![(&pmc_sapling, 50_000, None)]).await {
            Ok(_) => panic!(),
            Err(QuickSendError::ProposeSend(proposesenderror)) => match proposesenderror {
                ProposeSendError::Proposal(insufficient) => match insufficient {
                    zcash_client_backend::data_api::error::Error::InsufficientFunds {
                        available,
                        required,
                    } => {
                        assert_eq!(available, NonNegativeAmount::from_u64(20_000).unwrap());
                        assert_eq!(required, NonNegativeAmount::from_u64(70_000).unwrap());
                    }
                    _ => {
                        panic!()
                    }
                },
                _ => panic!(),
            },
            _ => panic!(),
        }
        // End of 11 no change
        bump_and_check!(o: 10_000 s: 10_000 t: 465_000);
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 12 Orchard and Sapling demote all to transparent self-send
        //  t -> o
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    15_000 1t and 2o
        client.quick_shield().await.unwrap();
        bump_and_check!(o: 460_000 s: 10_000 t: 0);
        test_dev_total_expected_fee += 15_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 13 Orchard and Sapling demote all to transparent self-send
        //  o -> z
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    20_000 2o and 2s
        from_inputs::quick_send(&client, vec![(&pmc_sapling, 10_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 430_000 s: 20_000 t: 0);
        test_dev_total_expected_fee += 20_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 14 Orchard and Sapling demote all to transparent self-send
        //  o -> o
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    10_000
        from_inputs::quick_send(&client, vec![(&pmc_unified, 20_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 420_000 s: 20_000 t: 0);
        test_dev_total_expected_fee += 10_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 14 Orchard and Sapling demote all to transparent self-send
        //  zo -> o
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    20_000
        from_inputs::quick_send(&client, vec![(&pmc_sapling, 400_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 10_000 s: 410_000 t: 0);
        test_dev_total_expected_fee += 20_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 15 Orchard and Sapling demote all to transparent self-send
        //  z -> z
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    10_000  sapling->sapling change to sapling, with 2 or fewer inputs
        from_inputs::quick_send(&client, vec![(&pmc_sapling, 380_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 10_000 s: 400_000 t: 0);
        test_dev_total_expected_fee += 10_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        let total_fee = get_fees_paid_by_client(&client).await;
        assert_eq!(total_fee, test_dev_total_expected_fee);
    }

    #[tokio::test]
    async fn factor_do_shield_to_call_do_send() {
        let (regtest_manager, __cph, mut faucet, recipient) =
            scenarios::faucet_recipient_default().await;
        zingolib::testutils::increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 2)
            .await
            .unwrap();
        from_inputs::quick_send(
            &faucet,
            vec![(
                &get_base_address_macro!(recipient, "transparent"),
                1_000u64,
                None,
            )],
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn dust_sends_change_correctly() {
        let (_regtest_manager, _cph, faucet, recipient, _txid) =
            scenarios::faucet_funded_recipient_default(100_000).await;

        // Send of less that transaction fee
        let sent_value = 1_000;
        let _sent_transaction_id = from_inputs::quick_send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "unified"),
                sent_value,
                None,
            )],
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn by_address_finsight() {
        let (regtest_manager, _cph, mut faucet, recipient) =
            scenarios::faucet_recipient_default().await;
        let base_uaddress = get_base_address_macro!(recipient, "unified");
        zingolib::testutils::increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 2)
            .await
            .unwrap();
        from_inputs::quick_send(&faucet, vec![(&base_uaddress, 1_000u64, Some("1"))])
            .await
            .unwrap();
        from_inputs::quick_send(&faucet, vec![(&base_uaddress, 1_000u64, Some("1"))])
            .await
            .unwrap();
        assert_eq!(
            JsonValue::from(faucet.do_total_memobytes_to_address().await)[&base_uaddress].pretty(4),
            "2".to_string()
        );
        from_inputs::quick_send(&faucet, vec![(&base_uaddress, 1_000u64, Some("aaaa"))])
            .await
            .unwrap();
        assert_eq!(
            JsonValue::from(faucet.do_total_memobytes_to_address().await)[&base_uaddress].pretty(4),
            "6".to_string()
        );
    }

    #[tokio::test]
    async fn zero_value_change_to_orchard_created() {
        let (regtest_manager, _cph, faucet, mut recipient, _txid) =
            scenarios::faucet_funded_recipient_default(100_000).await;

        zingolib::testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &mut recipient,
            1,
        )
        .await
        .unwrap();

        // 1. Send a transaction to an external z addr
        let sent_zvalue = 80_000;
        let sent_zmemo = "Ext z";
        let sent_transaction_id = from_inputs::quick_send(
            &recipient,
            vec![(
                &get_base_address_macro!(faucet, "sapling"),
                sent_zvalue,
                Some(sent_zmemo),
            )],
        )
        .await
        .unwrap()
        .first()
        .to_string();

        // Validate transaction
        zingolib::testutils::increase_height_and_wait_for_client(
            &regtest_manager,
            &mut recipient,
            1,
        )
        .await
        .unwrap();

        let sent_txid = txid_from_hex_encoded_str(&sent_transaction_id).unwrap();
        let orchard_note = recipient
            .wallet
            .lock()
            .await
            .wallet_transactions
            .get(&sent_txid)
            .unwrap()
            .orchard_notes()
            .first()
            .unwrap()
            .clone();
        assert_eq!(orchard_note.value(), 0);
    }
    #[tokio::test]
    async fn mempool_spends_correctly_marked_pending_spent() {
        let (regtest_manager, _cph, faucet, mut recipient, _txid) =
            scenarios::faucet_funded_recipient_default(1_000_000).await;
        let sent_txids = from_inputs::quick_send(
            &recipient,
            vec![(&get_base_address_macro!(faucet, "sapling"), 100_000, None)],
        )
        .await
        .unwrap();
        recipient.sync_and_await(true).await.unwrap();
        {
            let recipient_wallet = recipient.wallet.lock().await;
            let sapling_notes = recipient_wallet.wallet_outputs::<SaplingNote>();
            assert_eq!(sapling_notes.len(), 0);
            let orchard_notes = recipient_wallet.wallet_outputs::<OrchardNote>();
            assert_eq!(orchard_notes.len(), 2);
            let spent_orchard_note = (*orchard_notes
                .iter()
                .find(|&&note| note.value() == 1_000_000)
                .unwrap())
            .clone();
            assert_eq!(
                recipient_wallet.output_spend_status(&spent_orchard_note),
                SpendStatus::MempoolSpent(*sent_txids.first())
            );
            let orchard_change_note = (*orchard_notes
                .iter()
                .find(|&&note| note.value() == 880_000)
                .unwrap())
            .clone();
            assert_eq!(
                recipient_wallet.output_spend_status(&orchard_change_note),
                SpendStatus::Unspent
            );
            assert!(!recipient_wallet
                .output_transaction(&orchard_change_note)
                .status()
                .is_confirmed());
        }
        let balance = recipient.do_balance().await;
        assert_eq!(balance.orchard_balance, Some(880_000));
        assert_eq!(balance.unverified_orchard_balance, Some(880_000));
        assert_eq!(balance.verified_orchard_balance, Some(0));
        increase_height_and_wait_for_client(&regtest_manager, &mut recipient, 1)
            .await
            .unwrap();
        {
            let recipient_wallet = recipient.wallet.lock().await;
            let sapling_notes = recipient_wallet.wallet_outputs::<SaplingNote>();
            assert_eq!(sapling_notes.len(), 0);
            let orchard_notes = recipient_wallet.wallet_outputs::<OrchardNote>();
            assert_eq!(orchard_notes.len(), 2);
            let spent_orchard_note = (*orchard_notes
                .iter()
                .find(|&&note| note.value() == 1_000_000)
                .unwrap())
            .clone();
            assert_eq!(
                recipient_wallet.output_spend_status(&spent_orchard_note),
                SpendStatus::Spent(*sent_txids.first())
            );
            let orchard_change_note = (*orchard_notes
                .iter()
                .find(|&&note| note.value() == 880_000)
                .unwrap())
            .clone();
            assert_eq!(
                recipient_wallet.output_spend_status(&orchard_change_note),
                SpendStatus::Unspent
            );
            assert!(recipient_wallet
                .output_transaction(&orchard_change_note)
                .status()
                .is_confirmed());
        }
        let balance = recipient.do_balance().await;
        assert_eq!(balance.orchard_balance, Some(880_000));
        assert_eq!(balance.unverified_orchard_balance, Some(0));
        assert_eq!(balance.verified_orchard_balance, Some(880_000));
    }
}

mod basic_transactions {
    use zingolib::get_base_address_macro;
    use zingolib::testutils::{lightclient::from_inputs, scenarios};

    #[tokio::test]
    async fn send_and_sync_with_multiple_notes_no_panic() {
        let (regtest_manager, _cph, mut faucet, mut recipient) =
            scenarios::faucet_recipient_default().await;

        let recipient_addr_ua = get_base_address_macro!(recipient, "unified");
        let faucet_addr_ua = get_base_address_macro!(faucet, "unified");

        zingolib::testutils::generate_n_blocks_return_new_height(&regtest_manager, 2)
            .await
            .unwrap();

        recipient.sync_and_await(true).await.unwrap();
        faucet.sync_and_await(true).await.unwrap();

        for _ in 0..2 {
            from_inputs::quick_send(&faucet, vec![(recipient_addr_ua.as_str(), 40_000, None)])
                .await
                .unwrap();
        }

        zingolib::testutils::generate_n_blocks_return_new_height(&regtest_manager, 1)
            .await
            .unwrap();

        recipient.sync_and_await(true).await.unwrap();
        faucet.sync_and_await(true).await.unwrap();

        from_inputs::quick_send(&recipient, vec![(faucet_addr_ua.as_str(), 50_000, None)])
            .await
            .unwrap();

        zingolib::testutils::generate_n_blocks_return_new_height(&regtest_manager, 1)
            .await
            .unwrap();

        recipient.sync_and_await(true).await.unwrap();
        faucet.sync_and_await(true).await.unwrap();
    }

    // FIXME: zingo2 rewrite action / inputs / outputs counting using new interface
    // #[tokio::test]
    // async fn standard_send_fees() {
    //     let (regtest_manager, _cph, faucet, recipient) =
    //         scenarios::faucet_recipient_default().await;

    //     let txid1 = from_inputs::quick_send(
    //         &faucet,
    //         vec![(
    //             get_base_address_macro!(recipient, "unified").as_str(),
    //             40_000,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap()
    //     .first()
    //     .to_string();

    //     let txid2 = from_inputs::quick_send(
    //         &faucet,
    //         vec![(
    //             get_base_address_macro!(recipient, "sapling").as_str(),
    //             40_000,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap()
    //     .first()
    //     .to_string();

    //     let txid3 = from_inputs::quick_send(
    //         &faucet,
    //         vec![(
    //             get_base_address_macro!(recipient, "transparent").as_str(),
    //             40_000,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap()
    //     .first()
    //     .to_string();

    //     zingolib::testutils::generate_n_blocks_return_new_height(&regtest_manager, 1)
    //         .await
    //         .unwrap();

    //     faucet.do_sync(true).await.unwrap();
    //     recipient.do_sync(true).await.unwrap();

    //     println!(
    //         "Transaction Inputs:\n{:?}",
    //         zingolib::testutils::tx_inputs(&faucet, txid1.as_str()).await
    //     );
    //     println!(
    //         "Transaction Outputs:\n{:?}",
    //         zingolib::testutils::tx_outputs(&recipient, txid1.as_str()).await
    //     );
    //     println!(
    //         "Transaction Change:\n{:?}",
    //         zingolib::testutils::tx_outputs(&faucet, txid1.as_str()).await
    //     );

    //     let tx_actions_txid1 =
    //         zingolib::testutils::tx_actions(&faucet, Some(&recipient), txid1.as_str()).await;
    //     println!("Transaction Actions:\n{:?}", tx_actions_txid1);

    //     let calculated_fee_txid1 =
    //         zingolib::testutils::total_tx_value(&faucet, txid1.as_str()).await - 40_000;
    //     println!("Fee Paid: {}", calculated_fee_txid1);

    //     let expected_fee_txid1 = 5000
    //         * (cmp::max(
    //             2,
    //             tx_actions_txid1.transparent_tx_actions
    //                 + tx_actions_txid1.sapling_tx_actions
    //                 + tx_actions_txid1.orchard_tx_actions,
    //         ));
    //     println!("Expected Fee: {}", expected_fee_txid1);

    //     assert_eq!(calculated_fee_txid1, expected_fee_txid1 as u64);

    //     println!(
    //         "Transaction Inputs:\n{:?}",
    //         zingolib::testutils::tx_inputs(&faucet, txid2.as_str()).await
    //     );
    //     println!(
    //         "Transaction Outputs:\n{:?}",
    //         zingolib::testutils::tx_outputs(&recipient, txid2.as_str()).await
    //     );
    //     println!(
    //         "Transaction Change:\n{:?}",
    //         zingolib::testutils::tx_outputs(&faucet, txid2.as_str()).await
    //     );

    //     let tx_actions_txid2 =
    //         zingolib::testutils::tx_actions(&faucet, Some(&recipient), txid2.as_str()).await;
    //     println!("Transaction Actions:\n{:?}", tx_actions_txid2);

    //     let calculated_fee_txid2 =
    //         zingolib::testutils::total_tx_value(&faucet, txid2.as_str()).await - 40_000;
    //     println!("Fee Paid: {}", calculated_fee_txid2);

    //     let expected_fee_txid2 = 5000
    //         * (cmp::max(
    //             2,
    //             tx_actions_txid2.transparent_tx_actions
    //                 + tx_actions_txid2.sapling_tx_actions
    //                 + tx_actions_txid2.orchard_tx_actions,
    //         ));
    //     println!("Expected Fee: {}", expected_fee_txid2);

    //     assert_eq!(calculated_fee_txid2, expected_fee_txid2 as u64);

    //     println!(
    //         "Transaction Inputs:\n{:?}",
    //         zingolib::testutils::tx_inputs(&faucet, txid3.as_str()).await
    //     );
    //     println!(
    //         "Transaction Outputs:\n{:?}",
    //         zingolib::testutils::tx_outputs(&recipient, txid3.as_str()).await
    //     );
    //     println!(
    //         "Transaction Change:\n{:?}",
    //         zingolib::testutils::tx_outputs(&faucet, txid3.as_str()).await
    //     );

    //     let tx_actions_txid3 =
    //         zingolib::testutils::tx_actions(&faucet, Some(&recipient), txid3.as_str()).await;
    //     println!("Transaction Actions:\n{:?}", tx_actions_txid3);

    //     let calculated_fee_txid3 =
    //         zingolib::testutils::total_tx_value(&faucet, txid3.as_str()).await - 40_000;
    //     println!("Fee Paid: {}", calculated_fee_txid3);

    //     let expected_fee_txid3 = 5000
    //         * (cmp::max(
    //             2,
    //             tx_actions_txid3.transparent_tx_actions
    //                 + tx_actions_txid3.sapling_tx_actions
    //                 + tx_actions_txid3.orchard_tx_actions,
    //         ));
    //     println!("Expected Fee: {}", expected_fee_txid3);

    //     assert_eq!(calculated_fee_txid3, expected_fee_txid3 as u64);

    //     let txid4 = zingolib::testutils::lightclient::from_inputs::quick_send(
    //         &recipient,
    //         vec![(
    //             get_base_address_macro!(faucet, "transparent").as_str(),
    //             55_000,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap()
    //     .first()
    //     .to_string();

    //     zingolib::testutils::generate_n_blocks_return_new_height(&regtest_manager, 1)
    //         .await
    //         .unwrap();

    //     faucet.do_sync(true).await.unwrap();
    //     recipient.do_sync(true).await.unwrap();

    //     println!(
    //         "Transaction Inputs:\n{:?}",
    //         zingolib::testutils::tx_inputs(&recipient, txid4.as_str()).await
    //     );
    //     println!(
    //         "Transaction Outputs:\n{:?}",
    //         zingolib::testutils::tx_outputs(&faucet, txid4.as_str()).await
    //     );
    //     println!(
    //         "Transaction Change:\n{:?}",
    //         zingolib::testutils::tx_outputs(&recipient, txid4.as_str()).await
    //     );

    //     let tx_actions_txid4 =
    //         zingolib::testutils::tx_actions(&recipient, Some(&faucet), txid4.as_str()).await;
    //     println!("Transaction Actions:\n{:?}", tx_actions_txid4);

    //     let calculated_fee_txid4 =
    //         zingolib::testutils::total_tx_value(&recipient, txid4.as_str()).await - 55_000;
    //     println!("Fee Paid: {}", calculated_fee_txid4);

    //     let expected_fee_txid4 = 5000
    //         * (cmp::max(
    //             2,
    //             tx_actions_txid4.transparent_tx_actions
    //                 + tx_actions_txid4.sapling_tx_actions
    //                 + tx_actions_txid4.orchard_tx_actions,
    //         ));
    //     println!("Expected Fee: {}", expected_fee_txid4);

    //     assert_eq!(calculated_fee_txid4, expected_fee_txid4 as u64);
    // }

    // #[tokio::test]
    // async fn dust_send_fees() {
    //     let (regtest_manager, _cph, faucet, recipient) =
    //         scenarios::faucet_recipient_default().await;

    //     let txid1 = zingolib::testutils::lightclient::from_inputs::quick_send(
    //         &faucet,
    //         vec![(
    //             get_base_address_macro!(recipient, "unified").as_str(),
    //             0,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap()
    //     .first()
    //     .to_string();

    //     zingolib::testutils::generate_n_blocks_return_new_height(&regtest_manager, 1)
    //         .await
    //         .unwrap();

    //     faucet.do_sync(true).await.unwrap();
    //     recipient.do_sync(true).await.unwrap();

    //     println!(
    //         "Transaction Inputs:\n{:?}",
    //         zingolib::testutils::tx_inputs(&faucet, txid1.as_str()).await
    //     );
    //     println!(
    //         "Transaction Outputs:\n{:?}",
    //         zingolib::testutils::tx_outputs(&recipient, txid1.as_str()).await
    //     );
    //     println!(
    //         "Transaction Change:\n{:?}",
    //         zingolib::testutils::tx_outputs(&faucet, txid1.as_str()).await
    //     );

    //     let tx_actions_txid1 =
    //         zingolib::testutils::tx_actions(&faucet, Some(&recipient), txid1.as_str()).await;
    //     println!("Transaction Actions:\n{:?}", tx_actions_txid1);

    //     let calculated_fee_txid1 =
    //         zingolib::testutils::total_tx_value(&faucet, txid1.as_str()).await;
    //     println!("Fee Paid: {}", calculated_fee_txid1);

    //     let expected_fee_txid1 = 5000
    //         * (cmp::max(
    //             2,
    //             tx_actions_txid1.transparent_tx_actions
    //                 + tx_actions_txid1.sapling_tx_actions
    //                 + tx_actions_txid1.orchard_tx_actions,
    //         ));
    //     println!("Expected Fee: {}", expected_fee_txid1);

    //     assert_eq!(calculated_fee_txid1, expected_fee_txid1 as u64);
    // }

    // #[tokio::test]
    // async fn shield_send_fees() {
    //     let (regtest_manager, _cph, faucet, recipient) =
    //         scenarios::faucet_recipient_default().await;

    //     zingolib::testutils::lightclient::from_inputs::quick_send(
    //         &faucet,
    //         vec![(
    //             get_base_address_macro!(recipient, "transparent").as_str(),
    //             40_000,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap();

    //     zingolib::testutils::generate_n_blocks_return_new_height(&regtest_manager, 1)
    //         .await
    //         .unwrap();

    //     faucet.do_sync(true).await.unwrap();
    //     recipient.do_sync(true).await.unwrap();

    //     let txid1 = recipient.quick_shield().await.unwrap().first().to_string();

    //     zingolib::testutils::generate_n_blocks_return_new_height(&regtest_manager, 1)
    //         .await
    //         .unwrap();

    //     faucet.do_sync(true).await.unwrap();
    //     recipient.do_sync(true).await.unwrap();

    //     println!(
    //         "Transaction Inputs:\n{:?}",
    //         zingolib::testutils::tx_inputs(&recipient, txid1.as_str()).await
    //     );
    //     println!(
    //         "Transaction Outputs:\n{:?}",
    //         zingolib::testutils::tx_outputs(&recipient, txid1.as_str()).await
    //     );

    //     let tx_actions_txid1 =
    //         zingolib::testutils::tx_actions(&recipient, None, txid1.as_str()).await;
    //     println!("Transaction Actions:\n{:?}", tx_actions_txid1);

    //     let calculated_fee_txid1 =
    //         zingolib::testutils::total_tx_value(&recipient, txid1.as_str()).await;
    //     println!("Fee Paid: {}", calculated_fee_txid1);

    //     let expected_fee_txid1 = 5000
    //         * (cmp::max(
    //             2,
    //             tx_actions_txid1.transparent_tx_actions
    //                 + tx_actions_txid1.sapling_tx_actions
    //                 + tx_actions_txid1.orchard_tx_actions,
    //         ));
    //     println!("Expected Fee: {}", expected_fee_txid1);

    //     assert_eq!(calculated_fee_txid1, expected_fee_txid1 as u64);

    //     zingolib::testutils::lightclient::from_inputs::quick_send(
    //         &faucet,
    //         vec![(
    //             get_base_address_macro!(recipient, "transparent").as_str(),
    //             40_000,
    //             None,
    //         )],
    //     )
    //     .await
    //     .unwrap();

    //     zingolib::testutils::generate_n_blocks_return_new_height(&regtest_manager, 1)
    //         .await
    //         .unwrap();

    //     faucet.do_sync(true).await.unwrap();
    //     recipient.do_sync(true).await.unwrap();
    // }
}
#[ignore = "flake"]
#[tokio::test]
async fn proxy_server_worky() {
    zingolib::testutils::check_proxy_server_works().await
}

// FIXME: does not assert dust was included in the proposal
#[tokio::test]
async fn propose_orchard_dust_to_sapling() {
    let (regtest_manager, _cph, faucet, mut recipient, _) =
        scenarios::faucet_funded_recipient_default(100_000).await;

    from_inputs::quick_send(
        &faucet,
        vec![(&get_base_address_macro!(&recipient, "unified"), 4_000, None)],
    )
    .await
    .unwrap();
    increase_height_and_wait_for_client(&regtest_manager, &mut recipient, 1)
        .await
        .unwrap();

    from_inputs::propose(
        &recipient,
        vec![(&get_base_address_macro!(faucet, "sapling"), 10_000, None)],
    )
    .await
    .unwrap();
}

mod send_all {

    use pepper_sync::wallet::{OrchardNote, SaplingNote};

    use super::*;
    #[tokio::test]
    async fn toggle_zennies_for_zingo() {
        let (regtest_manager, _cph, faucet, mut recipient) =
            scenarios::faucet_recipient_default().await;

        let initial_funds = 2_000_000;
        let zennies_magnitude = 1_000_000;
        let expected_fee = 15_000; // 1 orchard note in, and 3 out
        from_inputs::quick_send(
            &faucet,
            vec![(
                &get_base_address_macro!(&recipient, "unified"),
                initial_funds,
                None,
            )],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&regtest_manager, &mut recipient, 1)
            .await
            .unwrap();
        let external_uaddress =
            address_from_str(&get_base_address_macro!(faucet, "unified")).unwrap();
        let expected_balance =
            NonNegativeAmount::from_u64(initial_funds - zennies_magnitude - expected_fee).unwrap();
        assert_eq!(
            recipient
                .get_spendable_shielded_balance(external_uaddress, true)
                .await
                .unwrap(),
            expected_balance
        );
    }

    #[tokio::test]
    async fn ptfm_general() {
        let (regtest_manager, _cph, mut faucet, mut recipient, _) =
            scenarios::faucet_funded_recipient_default(100_000).await;

        from_inputs::quick_send(
            &faucet,
            vec![(&get_base_address_macro!(&recipient, "unified"), 5_000, None)],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 1)
            .await
            .unwrap();
        from_inputs::quick_send(
            &faucet,
            vec![(
                &get_base_address_macro!(&recipient, "sapling"),
                50_000,
                None,
            )],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 1)
            .await
            .unwrap();
        from_inputs::quick_send(
            &faucet,
            vec![(&get_base_address_macro!(&recipient, "sapling"), 4_000, None)],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 1)
            .await
            .unwrap();
        from_inputs::quick_send(
            &faucet,
            vec![(&get_base_address_macro!(&recipient, "unified"), 4_000, None)],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&regtest_manager, &mut faucet, 1)
            .await
            .unwrap();
        recipient.sync_and_await(true).await.unwrap();

        recipient
            .propose_send_all(
                address_from_str(&get_base_address_macro!(faucet, "sapling")).unwrap(),
                false,
                None,
            )
            .await
            .unwrap();
        recipient
            .complete_and_broadcast_stored_proposal()
            .await
            .unwrap();
        increase_height_and_wait_for_client(&regtest_manager, &mut recipient, 1)
            .await
            .unwrap();
        faucet.sync_and_await(true).await.unwrap();

        assert_eq!(
            recipient
                .wallet
                .lock()
                .await
                .confirmed_balance_excluding_dust::<SaplingNote>()
                .await,
            Some(0)
        );
        assert_eq!(
            recipient
                .wallet
                .lock()
                .await
                .confirmed_balance_excluding_dust::<OrchardNote>()
                .await,
            Some(0)
        );
    }

    #[tokio::test]
    async fn ptfm_insufficient_funds() {
        let (_regtest_manager, _cph, faucet, recipient, _) =
            scenarios::faucet_funded_recipient_default(10_000).await;

        let proposal_error = recipient
            .propose_send_all(
                address_from_str(&get_base_address_macro!(faucet, "sapling")).unwrap(),
                false,
                None,
            )
            .await;

        match proposal_error {
            Err(ProposeSendError::Proposal(
                zcash_client_backend::data_api::error::Error::InsufficientFunds {
                    available: a,
                    required: r,
                },
            )) => {
                assert_eq!(a, NonNegativeAmount::const_from_u64(10_000));
                assert_eq!(r, NonNegativeAmount::const_from_u64(20_000));
            }
            _ => panic!("expected an InsufficientFunds error"),
        }
    }

    #[tokio::test]
    async fn ptfm_zero_value() {
        let (_regtest_manager, _cph, faucet, recipient, _) =
            scenarios::faucet_funded_recipient_default(10_000).await;

        let proposal_error = recipient
            .propose_send_all(
                address_from_str(&get_base_address_macro!(faucet, "unified")).unwrap(),
                false,
                None,
            )
            .await;

        assert!(matches!(
            proposal_error,
            Err(ProposeSendError::ZeroValueSendAll)
        ))
    }
}

#![forbid(unsafe_code)]
#![cfg(feature = "unit_test_twins")]
//! Pre-migration originals of tests that gained offline unit twins.
//!
//! Each test here is the LIVE original of an offline twin in zingolib
//! (see docs/testing/live-offline-twins.md for the per-test equivalence
//! record). The originals are preserved verbatim but gated out of the
//! default suite: the `unit_test_twins` feature, this module, and this
//! file all carry the same name. Run them with
//! `cargo nextest run -p libtonode-tests --features unit_test_twins`.
//!
//! Two originals were deleted on 2026-07-21 rather than preserved:
//! `list_value_transfers_check_fees` and
//! `from_t_z_o_tz_to_zo_tzo_to_orchard`. Their bump-and-check macros
//! bit-rotted during the ironwood-era balance migration: each bound an
//! `i:` argument but expanded to `i: 0`, silently discarding every
//! ironwood expectation the call sites recorded. The offline twins in
//! `zingolib/src/lightclient/mock_chain_tests.rs` expand the argument
//! correctly and carry the same ledgers, so they are now the sole
//! record of both tests.
//!
//! The fast/slow module wrappers the originals once carried were
//! flattened out repository-wide (tests have one-shorter paths); each
//! test's historical identity, including its old module-qualified name,
//! is recorded in the equivalence table of
//! docs/testing/live-offline-twins.md.

mod unit_test_twins {
    use pepper_sync::wallet::OrchardNote;
    use zcash_primitives::transaction::fees::zip317::{MARGINAL_FEE, MINIMUM_FEE};
    use zcash_protocol::PoolType;
    use zcash_protocol::consensus::{BlockHeight, COINBASE_MATURITY_BLOCKS};
    use zingo_status::confirmation_status::ConfirmationStatus;
    use zingo_test_vectors::TEST_TXID;
    use zingolib::testutils::lightclient::from_inputs;
    use zingolib::testutils::{
        assert_transaction_summary_equality, assert_transaction_summary_exists,
    };
    use zingolib::utils;
    use zingolib::wallet::output::SpendStatus;
    use zingolib::wallet::summary;
    use zingolib::wallet::summary::data::{
        BasicNoteSummary, OutgoingNoteSummary, SendType, SentValueTransfer, TransactionKind,
        TransactionSummary, ValueTransferKind,
    };
    use zingolib::{check_client_balances, get_base_address_macro};
    use zingolib_testutils::scenarios::{self, increase_height_and_wait_for_client};
    use zip32::AccountId;

    #[tokio::test]
    async fn mine_to_transparent_and_shield() {
        let activation_heights = scenarios::default_test_activation_heights();
        let (local_net, mut faucet, _recipient) = scenarios::faucet_recipient(
            PoolType::Transparent,
            activation_heights,
            scenarios::ChainCachePolicy::PerTest,
        )
        .await;
        increase_height_and_wait_for_client(&local_net, &mut faucet, COINBASE_MATURITY_BLOCKS)
            .await
            .unwrap();
        faucet.quick_shield(zip32::AccountId::ZERO).await.unwrap();
        increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
            .await
            .unwrap();

        assert_eq!(
            faucet
                .account_balance(zip32::AccountId::ZERO)
                .await
                .unwrap()
                .confirmed_orchard_balance
                .unwrap()
                .into_u64(),
            // 4 mature coinbases shielded in one step, minus the shield fee.
            scenarios::mined_block_rewards_total(4) - 30_000
        );
    }

    #[tokio::test]
    async fn zero_value_receipts() {
        let (local_net, mut faucet, mut recipient, _txid) =
            scenarios::faucet_funded_recipient_default(100_000).await;

        let sent_value = 0;
        let _sent_transaction_id = from_inputs::quick_send(
            &mut faucet,
            vec![(
                &get_base_address_macro!(recipient, "unified"),
                sent_value,
                None,
            )],
        )
        .await
        .unwrap();

        increase_height_and_wait_for_client(&local_net, &mut recipient, 5)
            .await
            .unwrap();
        let _sent_transaction_id = from_inputs::quick_send(
            &mut recipient,
            vec![(&get_base_address_macro!(faucet, "unified"), 1000, None)],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&local_net, &mut recipient, 5)
            .await
            .unwrap();

        // The zero-value receipt must not perturb spendable arithmetic:
        // the recipient holds the 100_000 funding note less the 1_000
        // payment and its 10_000 ZIP-317 fee (one orchard spend, two
        // logical actions).
        check_client_balances!(recipient, i: 0 o: 89_000 s: 0 t: 0);

        let value_transfers = recipient.value_transfers(true).await.unwrap();
        // The funding receipt.
        assert!(
            value_transfers
                .iter()
                .any(|vt| vt.kind == ValueTransferKind::Received && vt.value == 100_000)
        );
        // Pinned by observation rather than specification: the zero-value
        // receipt surfaces as a single Received transfer of zero value in
        // the orchard pool, carried without corruption.
        assert_eq!(
            value_transfers
                .iter()
                .filter(|vt| vt.kind == ValueTransferKind::Received
                    && vt.value == 0
                    && vt.pools_received == [PoolType::ORCHARD])
                .count(),
            1
        );
        // The subsequent spend proceeds unimpeded by the zero-value note.
        assert!(value_transfers.iter().any(|vt| {
            vt.kind == ValueTransferKind::Sent(SentValueTransfer::Send)
                && vt.value == 1_000
                && vt.transaction_fee == Some(10_000)
        }));
        assert_eq!(value_transfers.iter().count(), 3);
    }
    #[tokio::test]
    async fn send_to_transparent_and_sapling_maintain_balance() {
        // Receipt of orchard funds
        let recipient_initial_funds = 100_000_000;
        let (ref local_net, mut faucet, mut recipient, _txid) =
            scenarios::faucet_funded_recipient_default(recipient_initial_funds).await;

        let summary_orchard_receipt = TransactionSummary {
            txid: utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
            datetime: 0,
            status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(
                scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 2,
            )),
            blockheight: BlockHeight::from_u32(scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 2),
            kind: TransactionKind::Received,
            value: recipient_initial_funds,
            fee: Some(10_000),
            zec_price: None,
            pools_sent_from: vec![],
            ironwood_notes: vec![],
            orchard_notes: vec![BasicNoteSummary::from_parts(
                recipient_initial_funds,
                SpendStatus::Spent(
                    utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
                ),
                0,
                None,
            )],
            sapling_notes: vec![],
            transparent_coins: vec![],
            outgoing_ironwood_notes: vec![],
            outgoing_orchard_notes: vec![],
            outgoing_sapling_notes: vec![],
            outgoing_transparent_coins: vec![],
        };

        // Send to faucet (external) sapling
        let first_send_to_sapling = 20_000;
        from_inputs::quick_send(
            &mut recipient,
            vec![(
                &get_base_address_macro!(faucet, "sapling"),
                first_send_to_sapling,
                None,
            )],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(local_net, &mut recipient, 1)
            .await
            .unwrap();
        let summary_external_sapling = TransactionSummary {
            txid: utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
            datetime: 0,
            status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 3)),
            blockheight: BlockHeight::from_u32(scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 3),
            kind: TransactionKind::Sent(SendType::Send),
            value: first_send_to_sapling,
            fee: Some(20_000),
            zec_price: None,
            pools_sent_from: vec![PoolType::ORCHARD],
            ironwood_notes: vec![],
            orchard_notes: vec![BasicNoteSummary::from_parts(
                99_960_000,
                SpendStatus::TransmittedSpent(
                    utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
                ),
                0,
                None,
            )],
            sapling_notes: vec![],
            transparent_coins: vec![],
            outgoing_ironwood_notes: vec![],
            outgoing_orchard_notes: vec![],
            outgoing_sapling_notes: vec![OutgoingNoteSummary {
                 output_index: 0,
                 value: first_send_to_sapling,
                 memo: None,
                 recipient: "zregtestsapling1sa4rckrf4zs6ny3l3ljnezupacvxfnjjn90lpeaa4ddtjeyww2ypzqr3jxfsta3t8dn3jk8cm4f".to_string(),
                 recipient_unified_address: Some("uregtest183rtm3qhxxermx3nxwa706va0xnypt3td648tayetchlp28hue08vrcnwq02ryyk5rh3y0xhftay8a5ynjdg8kr3juq5x0d9ygd5ffht".to_string()),
                 account_id: AccountId::ZERO,
                 scope: summary::data::Scope::from(zip32::Scope::External),
             }],
            outgoing_transparent_coins: vec![],
        };

        // Send to faucet (external) transparent
        let first_send_to_transparent = 20_000;
        let summary_external_transparent = TransactionSummary {
            txid: utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
            datetime: 0,
            status: ConfirmationStatus::Transmitted(BlockHeight::from_u32(
                scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 4,
            )),
            blockheight: BlockHeight::from_u32(scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 4),
            kind: TransactionKind::Sent(SendType::Send),
            value: first_send_to_transparent,
            fee: Some(15_000),
            zec_price: None,
            pools_sent_from: vec![PoolType::ORCHARD],
            ironwood_notes: vec![],
            orchard_notes: vec![BasicNoteSummary::from_parts(
                99_925_000,
                SpendStatus::Unspent,
                0,
                None,
            )],
            sapling_notes: vec![],
            transparent_coins: vec![],
            outgoing_ironwood_notes: vec![],
            outgoing_orchard_notes: vec![],
            outgoing_sapling_notes: vec![],
            outgoing_transparent_coins: vec![],
        };

        from_inputs::quick_send(
            &mut recipient,
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
            &recipient.transaction_summaries(false).await.unwrap().0[0],
            &summary_orchard_receipt,
        );
        assert_transaction_summary_equality(
            &recipient.transaction_summaries(false).await.unwrap().0[1],
            &summary_external_sapling,
        );
        assert_transaction_summary_equality(
            &recipient.transaction_summaries(false).await.unwrap().0[2],
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
            let recipient_wallet = recipient.wallet().read().await;
            assert_eq!(
                recipient_wallet
                    .unconfirmed_balance::<OrchardNote>(zip32::AccountId::ZERO)
                    .unwrap(),
                expected_funds.try_into().unwrap()
            );
            //  (2) The balance is not yet verified
            assert_eq!(
                recipient_wallet
                    .confirmed_balance::<OrchardNote>(zip32::AccountId::ZERO)
                    .unwrap(),
                0.try_into().unwrap()
            );
        }

        increase_height_and_wait_for_client(local_net, &mut faucet, 1)
            .await
            .unwrap();

        let recipient_second_funding = 1_000_000;
        let summary_orchard_receipt_2 = TransactionSummary {
            txid: utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
            datetime: 0,
            status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(
                scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 5,
            )),
            blockheight: BlockHeight::from_u32(scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 5),
            kind: TransactionKind::Received,
            value: recipient_second_funding,
            // The observed zip317 fee of the faucet's second-wave funding
            // send on the Core stack: by this point the faucet's note pool
            // is fragmented by the earlier waves (and selection is
            // smallest-first), making this a four-logical-action
            // transaction. Under the old monolithic legacy-validator
            // funding it was two actions (10_000).
            fee: Some(20_000),
            zec_price: None,
            pools_sent_from: vec![],
            ironwood_notes: vec![],
            orchard_notes: vec![BasicNoteSummary::from_parts(
                recipient_second_funding,
                SpendStatus::Spent(
                    utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
                ),
                0,
                Some("Second wave incoming".to_string()),
            )],
            sapling_notes: vec![],
            transparent_coins: vec![],
            outgoing_ironwood_notes: vec![],
            outgoing_orchard_notes: vec![],
            outgoing_sapling_notes: vec![],
            outgoing_transparent_coins: vec![],
        };
        from_inputs::quick_send(
            &mut faucet,
            vec![(
                &get_base_address_macro!(recipient, "unified"),
                recipient_second_funding,
                Some("Second wave incoming"),
            )],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(local_net, &mut recipient, 1)
            .await
            .unwrap();

        // Send to external (faucet) transparent
        let second_send_to_transparent = 20_000;
        let summary_external_transparent_2 = TransactionSummary {
            txid: utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
            datetime: 0,
            status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(
                scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 6,
            )),
            blockheight: BlockHeight::from_u32(scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 6),
            kind: TransactionKind::Sent(SendType::Send),
            value: second_send_to_transparent,
            fee: Some(15_000),
            zec_price: None,
            pools_sent_from: vec![PoolType::ORCHARD],
            ironwood_notes: vec![],
            orchard_notes: vec![BasicNoteSummary::from_parts(
                965_000,
                SpendStatus::Spent(
                    utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
                ),
                0,
                None,
            )],
            sapling_notes: vec![],
            transparent_coins: vec![],
            outgoing_ironwood_notes: vec![],
            outgoing_orchard_notes: vec![],
            outgoing_sapling_notes: vec![],
            outgoing_transparent_coins: vec![],
        };
        from_inputs::quick_send(
            &mut recipient,
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
        let summary_external_sapling_2 =

TransactionSummary {
            txid: utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
            datetime: 0,
            status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 6)),
            blockheight: BlockHeight::from_u32(scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 6),
            kind: TransactionKind::Sent(SendType::Send),
            value: second_send_to_sapling,
            fee: Some(20_000),
            zec_price: None,
            pools_sent_from: vec![PoolType::ORCHARD],
            ironwood_notes: vec![],
            orchard_notes: vec![BasicNoteSummary::from_parts(
                99_885_000,
                SpendStatus::Unspent,
                0,
                None,
            )],
            sapling_notes: vec![],
            transparent_coins: vec![],
            outgoing_ironwood_notes: vec![],
            outgoing_orchard_notes: vec![],
            outgoing_sapling_notes: vec![OutgoingNoteSummary {
                output_index: 0,
                 value: second_send_to_sapling,
                memo: None,
                 recipient: "zregtestsapling1sa4rckrf4zs6ny3l3ljnezupacvxfnjjn90lpeaa4ddtjeyww2ypzqr3jxfsta3t8dn3jk8cm4f".to_string(),
                 recipient_unified_address: Some("uregtest183rtm3qhxxermx3nxwa706va0xnypt3td648tayetchlp28hue08vrcnwq02ryyk5rh3y0xhftay8a5ynjdg8kr3juq5x0d9ygd5ffht".to_string()),
                 account_id: AccountId::ZERO,
                 scope: summary::data::Scope::from(zip32::Scope::External),
            }],
            outgoing_transparent_coins: vec![],
        };
        from_inputs::quick_send(
            &mut recipient,
            vec![(
                &get_base_address_macro!(faucet, "sapling"),
                second_send_to_sapling,
                None,
            )],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(local_net, &mut recipient, 1)
            .await
            .unwrap();

        // Third external transparent
        let external_transparent_3 = 20_000;
        let summary_external_transparent_3 = TransactionSummary {
            txid: utils::conversion::txid_from_hex_encoded_str(TEST_TXID).unwrap(),
            datetime: 0,
            status: ConfirmationStatus::Confirmed(BlockHeight::from_u32(
                scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 7,
            )),
            blockheight: BlockHeight::from_u32(scenarios::FUNDED_FAUCET_SETUP_HEIGHT + 7),
            kind: TransactionKind::Sent(SendType::Send),
            value: external_transparent_3,
            fee: Some(15_000),
            zec_price: None,
            pools_sent_from: vec![PoolType::ORCHARD],
            ironwood_notes: vec![],
            orchard_notes: vec![BasicNoteSummary::from_parts(
                930_000,
                SpendStatus::Unspent,
                0,
                None,
            )],
            sapling_notes: vec![],
            transparent_coins: vec![],
            outgoing_ironwood_notes: vec![],
            outgoing_orchard_notes: vec![],
            outgoing_sapling_notes: vec![],
            outgoing_transparent_coins: vec![],
        };
        from_inputs::quick_send(
            &mut recipient,
            vec![(
                &get_base_address_macro!(faucet, "transparent"),
                external_transparent_3,
                None,
            )],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(local_net, &mut recipient, 1)
            .await
            .unwrap();

        // Final check
        assert_transaction_summary_equality(
            &recipient.transaction_summaries(false).await.unwrap().0[3],
            &summary_orchard_receipt_2,
        );
        assert_transaction_summary_exists(&recipient, &summary_external_transparent_2).await; // due to summaries of the same blockheight changing order
        assert_transaction_summary_exists(&recipient, &summary_external_sapling_2).await; // we check all summaries for these expected transactions
        assert_transaction_summary_equality(
            &recipient.transaction_summaries(false).await.unwrap().0[6],
            &summary_external_transparent_3,
        );
        let second_wave_expected_funds = expected_funds + recipient_second_funding
            - second_send_to_sapling
            - second_send_to_transparent
            - external_transparent_3
            - (5 * u64::from(MINIMUM_FEE));
        assert_eq!(
            recipient
                .wallet()
                .read()
                .await
                .confirmed_balance::<OrchardNote>(zip32::AccountId::ZERO)
                .unwrap(),
            second_wave_expected_funds.try_into().unwrap(),
        );
    }
    #[tokio::test]
    async fn self_send_to_t_displays_as_one_transaction() {
        let (local_net, mut faucet, mut recipient) = scenarios::faucet_recipient_default().await;
        let recipient_unified_address = get_base_address_macro!(recipient, "unified");
        let sent_value = 80_000;
        from_inputs::quick_send(
            &mut faucet,
            vec![(recipient_unified_address.as_str(), sent_value, None)],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
            .await
            .unwrap();
        let recipient_taddr = get_base_address_macro!(recipient, "transparent");
        let recipient_zaddr = get_base_address_macro!(recipient, "sapling");
        let sent_to_taddr_value = 5_000;
        let sent_to_zaddr_value = 11_000;
        let sent_to_self_orchard_value = 1_000;
        from_inputs::quick_send(
            &mut recipient,
            vec![(recipient_taddr.as_str(), sent_to_taddr_value, None)],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
            .await
            .unwrap();
        from_inputs::quick_send(
            &mut recipient,
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
        faucet.sync_and_await().await.unwrap();
        from_inputs::quick_send(
            &mut faucet,
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
        increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
            .await
            .unwrap();
        tracing::info!(
            "{}",
            json::stringify_pretty(recipient.transaction_summaries(false).await.unwrap(), 4)
        );
        let mut txids = recipient
            .transaction_summaries(false)
            .await
            .unwrap()
            .txids()
            .into_iter();
        assert!(itertools::Itertools::all_unique(&mut txids));
    }
    #[tokio::test]
    async fn sapling_dust_fee_collection() {
        let (local_net, mut faucet, mut recipient) = scenarios::faucet_recipient_default().await;
        let recipient_sapling = get_base_address_macro!(recipient, "sapling");
        let recipient_unified = get_base_address_macro!(recipient, "unified");
        check_client_balances!(recipient, i: 0 o: 0 s: 0 t: 0);
        let fee = u64::from(MINIMUM_FEE);
        let for_orchard = dbg!(fee * 10);
        let for_sapling = dbg!(fee / 10);
        from_inputs::quick_send(
            &mut faucet,
            vec![
                (&recipient_unified, for_orchard, Some("Plenty for orchard.")),
                (&recipient_sapling, for_sapling, Some("Dust for sapling.")),
            ],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
            .await
            .unwrap();
        check_client_balances!(recipient, i: 0 o: for_orchard s: 0 t: 0 );

        from_inputs::quick_send(
            &mut recipient,
            vec![(
                &get_base_address_macro!(faucet, "unified"),
                fee * 5,
                Some("Five times fee."),
            )],
        )
        .await
        .unwrap();
        increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
            .await
            .unwrap();
        let remaining_orchard = for_orchard - (6 * fee);
        check_client_balances!(recipient, i: 0 o: remaining_orchard s: 0 t: 0);
    }

    mod basic_transactions {
        use super::*;

        #[tokio::test]
        async fn send_and_sync_with_multiple_notes_no_panic() {
            let (local_net, mut faucet, mut recipient) =
                scenarios::faucet_recipient_default().await;

            let recipient_addr_ua = get_base_address_macro!(recipient, "unified");
            let faucet_addr_ua = get_base_address_macro!(faucet, "unified");

            increase_height_and_wait_for_client(&local_net, &mut recipient, 2)
                .await
                .unwrap();
            scenarios::sync_client_to_validator_tip(&local_net, &mut faucet).await;

            for _ in 0..2 {
                from_inputs::quick_send(
                    &mut faucet,
                    vec![(recipient_addr_ua.as_str(), 40_000, None)],
                )
                .await
                .unwrap();
            }

            increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
                .await
                .unwrap();
            scenarios::sync_client_to_validator_tip(&local_net, &mut faucet).await;

            from_inputs::quick_send(
                &mut recipient,
                vec![(faucet_addr_ua.as_str(), 50_000, None)],
            )
            .await
            .unwrap();

            increase_height_and_wait_for_client(&local_net, &mut recipient, 1)
                .await
                .unwrap();
            scenarios::sync_client_to_validator_tip(&local_net, &mut faucet).await;

            // The 50_000 payment plus its 10_000 ZIP-317 fee exceeds either
            // 40_000 note alone, so the send consumed both and returned
            // 20_000 as change: the arithmetic survived the multi-input spend.
            check_client_balances!(recipient, i: 0 o: 20_000 s: 0 t: 0);
        }
    }
}

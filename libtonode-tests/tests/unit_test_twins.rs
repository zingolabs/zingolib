#![forbid(unsafe_code)]
#![cfg(feature = "unit_test_twins")]
//! Pre-migration originals of tests that gained offline unit twins.
//!
//! Each test here is the LIVE original of an offline twin in zingolib
//! (see docs/testing/regtest-removal-ledger.md for the per-test equivalence
//! record). The originals are preserved verbatim, never deleted, but
//! gated out of the default suite: the `unit_test_twins` feature, this
//! module, and this file all carry the same name. Run them with
//! `cargo nextest run -p libtonode-tests --features unit_test_twins`.
//!
//! The fast/slow module wrappers the originals once carried were
//! flattened out repository-wide (tests have one-shorter paths). Each
//! test's historical identity, including its old module-qualified name,
//! is recorded in the equivalence table of
//! docs/testing/regtest-removal-ledger.md.
//!
//! The bump-and-check macros of `list_value_transfers_check_fees` and
//! `from_t_z_o_tz_to_zo_tzo_to_orchard` bit-rotted during the
//! ironwood-era balance migration (each bound an `i:` argument but
//! expanded `i: 0`) and were repaired on 2026-07-21 (review of PR
//! #2495). Their ledgers were then adjudicated by live container runs
//! the same day: the chain confirmed the twins' fee model (no
//! orchard-bundle-view charge on V6 ironwood spends), and from
//! `from_t_z_o`'s step 10 the live ledger deliberately forks from the
//! twin's (the live proposer drains single-pool and refuses exact
//! drains). See docs/testing/regtest-removal-ledger.md before editing
//! either side.

mod unit_test_twins {
    use pepper_sync::wallet::IronwoodNote;
    use zcash_primitives::transaction::fees::zip317::{MARGINAL_FEE, MINIMUM_FEE};
    use zcash_protocol::PoolType;
    use zcash_protocol::consensus::{BlockHeight, COINBASE_MATURITY_BLOCKS};
    use zcash_protocol::value::Zatoshis;
    use zingo_status::confirmation_status::ConfirmationStatus;
    use zingo_test_vectors::TEST_TXID;
    use zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;
    use zingolib::config::WalletConfig;
    use zingolib::lightclient::error::{LightClientError, SendError};
    use zingolib::perspective::value_transfer::{SentValueTransfer, ValueTransferKind};
    use zingolib::testutils::lightclient::{from_inputs, get_fees_paid_by_client};
    use zingolib::testutils::{
        assert_transaction_summary_equality, assert_transaction_summary_exists,
        default_test_wallet_settings,
    };
    use zingolib::utils;
    use zingolib::wallet::error::ProposeSendError;
    use zingolib::wallet::output::SpendStatus;
    use zingolib::wallet::summary;
    use zingolib::wallet::summary::data::{
        BasicNoteSummary, OutgoingNoteSummary, SendType, TransactionKind, TransactionSummary,
    };
    use zingolib::{check_client_balances, get_base_address_macro};
    use zingolib_testutils::scenarios::{self, increase_height_and_wait_for_client};
    use zip32::AccountId;

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
        check_client_balances!(recipient, i: for_orchard o: 0 s: 0 t: 0 );

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
        let remaining_ironwood = for_orchard - (6 * fee);
        check_client_balances!(recipient, i: remaining_ironwood o: 0 s: 0 t: 0);
    }
    #[tokio::test]
    async fn from_t_z_o_tz_to_zo_tzo_to_orchard() {
        // Test all possible promoting note source combinations
        let (local_net, mut client_builder) = scenarios::custom_clients_default().await;
        let mut faucet = client_builder.build_faucet(false).await;
        let mut client = client_builder
            .build_client(
                WalletConfig::MnemonicPhrase {
                    mnemonic_phrase: HOSPITAL_MUSEUM_SEED.to_string(),
                    no_of_accounts: 1.try_into().unwrap(),
                    birthday: 1,
                    wallet_settings: default_test_wallet_settings(),
                },
                false,
            )
            .await;
        let pmc_taddr = get_base_address_macro!(client, "transparent");
        let pmc_sapling = get_base_address_macro!(client, "sapling");
        let pmc_unified = get_base_address_macro!(client, "unified");

        // Ensure that the faucet has confirmed spendable funds
        increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
            .await
            .unwrap();

        macro_rules! bump_and_check {
            (o: $o:tt i: $i:tt s: $s:tt t: $t:tt) => {
                increase_height_and_wait_for_client(&local_net, &mut client, 1).await.unwrap();
                check_client_balances!(client, i: $i o:$o s:$s t:$t);
            };
        }

        let mut test_dev_total_expected_fee = 0;
        // 1 pmc receives 50_000 transparent
        //  # Expected Fees to recipient:
        //    - legacy: 0
        //    - 317:    0
        from_inputs::quick_send(&mut faucet, vec![(&pmc_taddr, 50_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 0 i: 0 s: 0 t: 50_000);
        assert_eq!(test_dev_total_expected_fee, 0);

        // 2 pmc shields 50_000 transparent, to orchard paying fee
        //  t -> o
        //  # Expected Fees to recipient:
        //    - legacy: 10_000
        //    - 317:    15_000 = 1 transparent in + the padded ironwood pair
        //      (a V6 shield's change lands in the ironwood bundle, ADR 0009)
        client.quick_shield(zip32::AccountId::ZERO).await.unwrap();
        bump_and_check!(o: 0 i: 35_000 s: 0 t: 0);
        test_dev_total_expected_fee += 15_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 3 pmc receives 50_000 sapling
        //  # Expected Fees to recipient:
        //    - legacy: 0
        //    - 317:    0
        from_inputs::quick_send(&mut faucet, vec![(&pmc_sapling, 50_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 0 i: 35_000 s: 50_000 t: 0);
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 4 pmc migrates 50_000 from sapling to ironwood plus fee
        //  z -> i
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    20_000 = the sapling pair (spend plus its zero-value
        //      change: V6 keeps change in sapling when no orchard flow
        //      exists) + the ironwood payment pair. The selector widens
        //      past the 35_000 ironwood note to the sapling note and then
        //      uses sapling alone.
        from_inputs::quick_send(&mut client, vec![(&pmc_unified, 30_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 0 i: 65_000 s: 0 t: 0);
        test_dev_total_expected_fee += 20_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 5 Ironwood self-send of 55_000 (the pre-V6 ledger's amount, which
        //   fits: adjudicated live 2026-07-21, a V6 ironwood spend carries
        //   no separate orchard-bundle-view charge).
        //  i -> i
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    10_000 (the ironwood pair)
        from_inputs::quick_send(&mut client, vec![(&pmc_unified, 55_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 0 i: 55_000 s: 0 t: 0);
        test_dev_total_expected_fee += 10_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 6 to transparent and sapling from ironwood
        //  i -> tz
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    5_000 transparent out + 10_000 sapling pair +
        //      10_000 ironwood change pair == 25_000
        from_inputs::quick_send(
            &mut client,
            vec![(&pmc_taddr, 10_000, None), (&pmc_sapling, 10_000, None)],
        )
        .await
        .unwrap();
        bump_and_check!(o: 0 i: 10_000 s: 10_000 t: 10_000);
        test_dev_total_expected_fee += 25_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 7 Receive 500_000 to transparent
        from_inputs::quick_send(&mut faucet, vec![(&pmc_taddr, 500_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 0 i: 10_000 s: 10_000 t: 510_000);
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 8 Shield transparent to orchard
        //  t -> o
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    20_000 = 10_000 for the two transparent inputs +
        //      10_000 for the ironwood pair receiving the shielded value
        client.quick_shield(zip32::AccountId::ZERO).await.unwrap();
        bump_and_check!(o: 0 i: 500_000 s: 10_000 t: 0);
        test_dev_total_expected_fee += 20_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 9 self send ironwood to ironwood
        // TODO: already tested!?
        //  i -> i
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    10_000 (the ironwood pair)
        from_inputs::quick_send(&mut client, vec![(&pmc_unified, 30_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 0 i: 490_000 s: 10_000 t: 0);
        test_dev_total_expected_fee += 10_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 10 Ironwood demoted to transparent self-send.
        //  i -> t
        //  # Expected Fees:
        //    - 317: 15_000 = 5_000 transparent out + 10_000 ironwood pair.
        //  Adjudicated live 2026-07-21: the live proposer selects ironwood
        //  alone here (sapling's 10_000 stays put), and the mock's exact
        //  two-pool drain of 470_000 is refused at the boundary. Exact
        //  drains are pricing-shape-sensitive on the live proposer, so
        //  this ledger stays 5_000 inside the achievable maximum.
        from_inputs::quick_send(&mut client, vec![(&pmc_taddr, 465_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 0 i: 10_000 s: 10_000 t: 465_000);
        test_dev_total_expected_fee += 15_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 10 transparent to transparent
        // 10b transparent to transparent: refused, transparent funds are
        //     not send-spendable. The shielded leftovers (i: 10_000 +
        //     s: 10_000) are what the proposer offers against the
        //     10_000 payment + 25_000 fee it prices.
        match from_inputs::quick_send(&mut client, vec![(&pmc_taddr, 10_000, None)]).await {
            Ok(_) => panic!(),
            Err(LightClientError::SendError(SendError::ProposeSendError(e))) => match e {
                ProposeSendError::Proposal(insufficient) => {
                    if let zcash_client_backend::data_api::error::Error::InsufficientFunds {
                        available,
                        required,
                    } = insufficient
                    {
                        assert_eq!(available, Zatoshis::from_u64(20_000).unwrap());
                        assert_eq!(required, Zatoshis::from_u64(35_000).unwrap());
                    } else {
                        panic!()
                    }
                }
                ProposeSendError::TransactionRequestFailed(_) => panic!(),
            },
            _ => panic!(),
        }
        bump_and_check!(o: 0 i: 10_000 s: 10_000 t: 465_000);
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 11 transparent to sapling: likewise refused (50_000 payment +
        //    20_000 fee against the 20_000 shielded leftovers).
        //  t -> z
        match from_inputs::quick_send(&mut client, vec![(&pmc_sapling, 50_000, None)]).await {
            Ok(_) => panic!(),
            Err(LightClientError::SendError(SendError::ProposeSendError(e))) => {
                if let ProposeSendError::Proposal(insufficient_funds) = e {
                    match insufficient_funds {
                        zcash_client_backend::data_api::error::Error::InsufficientFunds {
                            available,
                            required,
                        } => {
                            assert_eq!(available, Zatoshis::from_u64(20_000).unwrap());
                            assert_eq!(required, Zatoshis::from_u64(70_000).unwrap());
                        }
                        _ => {
                            panic!()
                        }
                    }
                } else {
                    panic!()
                }
            }
            _ => panic!(),
        }
        bump_and_check!(o: 0 i: 10_000 s: 10_000 t: 465_000);
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 12 Shield
        //  t -> o
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    15_000 = 1 transparent in + the ironwood pair
        client.quick_shield(zip32::AccountId::ZERO).await.unwrap();
        bump_and_check!(o: 0 i: 460_000 s: 10_000 t: 0);
        test_dev_total_expected_fee += 15_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 13 Ironwood to Sapling
        //  i -> z
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    20_000 = the sapling payment pair + the ironwood
        //      change pair
        from_inputs::quick_send(&mut client, vec![(&pmc_sapling, 10_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 0 i: 430_000 s: 20_000 t: 0);
        test_dev_total_expected_fee += 20_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 14 Ironwood self-send
        //  i -> i
        // TODO: already tested!?
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    10_000 (the ironwood pair)
        from_inputs::quick_send(&mut client, vec![(&pmc_unified, 20_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 0 i: 420_000 s: 20_000 t: 0);
        test_dev_total_expected_fee += 10_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 15 Ironwood and Sapling to Sapling: the sapling-destination
        //    gather starts from sapling and widens to the ironwood notes.
        //    Not an exact drain: exact drains are pricing-shape-sensitive
        //    on the live proposer (see step 10), so this ledger keeps
        //    headroom (adjudicated live 2026-07-21).
        //  zi -> z
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    20_000 = the sapling pair + the ironwood spend pair
        from_inputs::quick_send(&mut client, vec![(&pmc_sapling, 400_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 0 i: 20_000 s: 400_000 t: 0);
        test_dev_total_expected_fee += 20_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );

        // 16 Sapling self-send
        //  z -> z
        //  # Expected Fees:
        //    - legacy: 10_000
        //    - 317:    10_000 (single sapling bundle: V6 change stays in
        //      sapling when no orchard flow exists)
        from_inputs::quick_send(&mut client, vec![(&pmc_sapling, 350_000, None)])
            .await
            .unwrap();
        bump_and_check!(o: 0 i: 20_000 s: 390_000 t: 0);
        test_dev_total_expected_fee += 10_000;
        assert_eq!(
            get_fees_paid_by_client(&client).await,
            test_dev_total_expected_fee
        );
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
            // 20_000 as change: the arithmetic survived the multi-input
            // spend. V6 receipts and change land in the ironwood pool.
            check_client_balances!(recipient, i: 20_000 o: 0 s: 0 t: 0);
        }
    }
}

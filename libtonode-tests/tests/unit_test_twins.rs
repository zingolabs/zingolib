#![forbid(unsafe_code)]
#![cfg(feature = "unit_test_twins")]
//! Live originals of tests that gained offline unit twins.
//!
//! Each twin's body lives ONCE in `zingolib::testutils::twin_fixtures`,
//! generic over the environment (user direction, 2026-07-09: "DRY them
//! maximally"); this module instantiates the fixtures against the real
//! LocalNet stack via `LiveTwinChain`, and
//! `zingolib::lightclient::mock_chain_tests` instantiates the same
//! fixtures offline. The live instantiations remain the control group,
//! gated out of the default suite: run them with
//! `cargo nextest run -p libtonode-tests --features unit_test_twins`.
//! Per-test equivalence history, including each test's old
//! module-qualified name, lives in docs/testing/live-offline-twins.md.
//!
//! Tests below the fixture stubs are LIVE-ONLY (no offline twin yet):
//! their bodies still live here.

mod unit_test_twins {
    use zcash_primitives::transaction::fees::zip317::MINIMUM_FEE;
    use zcash_protocol::PoolType;
    use zcash_protocol::consensus::COINBASE_MATURITY_BLOCKS;

    use libtonode_tests::twin_chain::LiveTwinChain;
    use zingolib::testutils::lightclient::from_inputs;
    use zingolib::testutils::twin_fixtures;
    use zingolib::{check_client_balances, get_base_address_macro};
    use zingolib_testutils::scenarios::{self, increase_height_and_wait_for_client};

    #[tokio::test]
    async fn zero_value_receipts() {
        twin_fixtures::zero_value_receipts::<LiveTwinChain>().await;
    }

    #[tokio::test]
    async fn list_value_transfers_check_fees() {
        twin_fixtures::list_value_transfers_check_fees::<LiveTwinChain>().await;
    }

    #[tokio::test]
    async fn self_send_to_t_displays_as_one_transaction() {
        twin_fixtures::self_send_to_t_displays_as_one_transaction::<LiveTwinChain>().await;
    }

    #[tokio::test]
    async fn send_to_transparent_and_sapling_maintain_balance() {
        twin_fixtures::send_to_transparent_and_sapling_maintain_balance::<LiveTwinChain>().await;
    }

    // NOTE: while the offline from_t_z_o twin is ignored on
    // zingolabs/zingolib#2447, the DEFAULT-SUITE copy of this protection
    // lives in concrete.rs (same fixture, same live environment); this
    // gated instantiation is the twin-contract control alongside it.
    #[tokio::test]
    async fn from_t_z_o_tz_to_zo_tzo_to_orchard() {
        twin_fixtures::from_t_z_o_tz_to_zo_tzo_to_orchard::<LiveTwinChain>().await;
    }

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
    async fn sapling_dust_fee_collection() {
        let (local_net, mut faucet, mut recipient) = scenarios::faucet_recipient_default().await;
        let recipient_sapling = get_base_address_macro!(recipient, "sapling");
        let recipient_unified = get_base_address_macro!(recipient, "unified");
        check_client_balances!(recipient, o: 0 s: 0 t: 0);
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
        check_client_balances!(recipient, o: for_orchard s: 0 t: 0 );

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
        check_client_balances!(recipient, o: remaining_orchard s: 0 t: 0);
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
            check_client_balances!(recipient, o: 20_000 s: 0 t: 0);
        }
    }
}

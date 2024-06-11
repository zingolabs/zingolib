//! tests that can be run either as lib-to-node or darkside.

//! this mod tests fair weather behavior i.e. the LightClient is connected to a server that provides expected responses about the state of the blockchain.
//! there are many ways to mock the chain. for simplicity, and in order to be usable in multiple contexts, the test fixtures in this mod delegate setup and server management to a ChainConductor (anything that implements ConductChain)
//

/// A Lightclient test may involve hosting a server to send data to the LightClient. This trait can be asked to set simple scenarios where a mock LightServer sends data showing a note to a LightClient, the LightClient updates and responds by sending the note, and the Lightserver accepts the transaction and rebroadcasts it...
/// The initial two implementors are
/// lib-to-node, which links a lightserver to a zcashd in regtest mode. see `impl ConductChain for LibtoNode
/// darkside, a mode for the lightserver which mocks zcashd. search 'impl ConductChain for DarksideScenario
pub mod conduct_chain {
    use crate::{get_base_address_macro, lightclient::from_inputs};
    use zingolib::lightclient::LightClient;

    #[allow(async_fn_in_trait)]
    #[allow(opaque_hidden_inferred_bound)]
    /// a trait (capability) for operating a server.
    /// delegates client setup, because different mock servers require different client configuration
    /// currently, the server conductor is limited to adding to the mock blockchain linearly (bump chain)
    pub trait ConductChain {
        /// set up the test chain
        async fn setup() -> Self;
        /// builds a faucet (funded from mining)
        async fn create_faucet(&mut self) -> LightClient;
        /// builds an empty client
        async fn create_client(&mut self) -> LightClient;

        /// moves the chain tip forward, creating 1 new block
        /// and confirming transactions that were received by the server
        async fn bump_chain(&mut self);

        /// builds a client and funds it in orchard and syncs it
        async fn fund_client_orchard(&mut self, value: u64) -> LightClient {
            let faucet = self.create_faucet().await;
            let recipient = self.create_client().await;

            self.bump_chain().await;
            faucet.do_sync(false).await.unwrap();

            from_inputs::quick_send(
                &faucet,
                vec![(
                    (get_base_address_macro!(recipient, "unified")).as_str(),
                    value,
                    None,
                )],
            )
            .await
            .unwrap();

            self.bump_chain().await;

            recipient.do_sync(false).await.unwrap();

            recipient
        }
    }
}

/// these functions are each meant to be 'test-in-a-box'
/// simply plug in a mock server as a chain conductor and provide some values
pub mod fixtures {
    use zcash_client_backend::PoolType;
    use zcash_client_backend::PoolType::Shielded;
    use zcash_client_backend::PoolType::Transparent;
    use zcash_client_backend::ShieldedProtocol::Orchard;
    use zcash_client_backend::ShieldedProtocol::Sapling;
    use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;

    use zingolib::wallet::notes::query::OutputPoolQuery;
    use zingolib::wallet::notes::query::OutputQuery;
    use zingolib::wallet::notes::query::OutputSpendStatusQuery;

    use crate::chain_generic_tests::conduct_chain::ConductChain;
    use crate::lightclient::from_inputs;
    use crate::lightclient::get_base_address;
    use crate::lightclient::with_assertions;

    /// runs a send-to-receiver and receives it in a chain-generic context
    pub async fn propose_and_broadcast_value_to_pool<CC>(send_value: u64, pooltype: PoolType)
    where
        CC: ConductChain,
    {
        let mut environment = CC::setup().await;

        println!("chain set up, funding client now");

        let expected_fee = MARGINAL_FEE.into_u64()
            * match pooltype {
                // contribution_transparent = 1
                //  1 transfer
                // contribution_orchard = 2
                //  1 input
                //  1 dummy output
                Transparent => 3,
                // contribution_sapling = 2
                //  1 output
                //  1 dummy input
                // contribution_orchard = 2
                //  1 input
                //  1 dummy output
                Shielded(Sapling) => 4,
                // contribution_orchard = 2
                //  1 input
                //  1 output
                Shielded(Orchard) => 2,
            };

        let sender = environment
            .fund_client_orchard(send_value + expected_fee)
            .await;

        println!("client is ready to send");

        let recipient = environment.create_client().await;

        println!("recipient ready");

        let recorded_fee = with_assertions::propose_send_bump_sync_recipient(
            &mut environment,
            &sender,
            &recipient,
            vec![(pooltype, send_value)],
        )
        .await;

        assert_eq!(expected_fee, recorded_fee);
    }

    /// required change should be 0
    pub async fn change_required<CC>()
    where
        CC: ConductChain,
    {
        let mut environment = CC::setup().await;
        let primary = environment.fund_client_orchard(45_000).await;
        let secondary = environment.create_client().await;

        assert_eq!(
            with_assertions::propose_send_bump_sync_recipient(
                &mut environment,
                &primary,
                &secondary,
                vec![(Shielded(Orchard), 1), (Shielded(Orchard), 29_999)]
            )
            .await,
            3 * MARGINAL_FEE.into_u64()
        );
    }

    /// sends back and forth several times, including sends to transparent
    pub async fn send_shield_cycle<CC>(n: u64)
    where
        CC: ConductChain,
    {
        let mut environment = CC::setup().await;
        let primary_fund = 1_000_000;
        let primary = environment.fund_client_orchard(primary_fund).await;

        let secondary = environment.create_client().await;

        for _ in 0..n {
            assert_eq!(
                with_assertions::propose_send_bump_sync_recipient(
                    &mut environment,
                    &primary,
                    &secondary,
                    vec![(Transparent, 100_000), (Transparent, 4_000)],
                )
                .await,
                MARGINAL_FEE.into_u64() * 4
            );

            assert_eq!(
                with_assertions::propose_shield_bump_sync(&mut environment, &secondary).await,
                MARGINAL_FEE.into_u64() * 3
            );

            assert_eq!(
                with_assertions::propose_send_bump_sync_recipient(
                    &mut environment,
                    &secondary,
                    &primary,
                    vec![(Shielded(Orchard), 50_000)],
                )
                .await,
                MARGINAL_FEE.into_u64() * 2
            );
        }
    }

    /// uses a dust input to pad another input to finish a transaction
    pub async fn send_required_dust<CC>()
    where
        CC: ConductChain,
    {
        let mut environment = CC::setup().await;
        let primary = environment.fund_client_orchard(120_000).await;
        let secondary = environment.create_client().await;

        assert_eq!(
            with_assertions::propose_send_bump_sync_recipient(
                &mut environment,
                &primary,
                &secondary,
                vec![(Shielded(Orchard), 1), (Shielded(Orchard), 99_999)]
            )
            .await,
            3 * MARGINAL_FEE.into_u64()
        );

        assert_eq!(
            with_assertions::propose_send_bump_sync_recipient(
                &mut environment,
                &secondary,
                &primary,
                vec![(Shielded(Orchard), 90_000)]
            )
            .await,
            2 * MARGINAL_FEE.into_u64()
        );
    }

    /// uses a dust input to pad another input to finish a transaction
    pub async fn send_grace_dust<CC>()
    where
        CC: ConductChain,
    {
        let mut environment = CC::setup().await;
        let primary = environment.fund_client_orchard(120_000).await;
        let secondary = environment.create_client().await;

        assert_eq!(
            with_assertions::propose_send_bump_sync_recipient(
                &mut environment,
                &primary,
                &secondary,
                vec![(Shielded(Orchard), 1), (Shielded(Orchard), 99_999)]
            )
            .await,
            3 * MARGINAL_FEE.into_u64()
        );

        assert_eq!(
            with_assertions::propose_send_bump_sync_recipient(
                &mut environment,
                &secondary,
                &primary,
                vec![(Shielded(Orchard), 30_000)]
            )
            .await,
            2 * MARGINAL_FEE.into_u64()
        );

        // since we used our dust as a freebie in the last send, we should only have 2
        assert_eq!(
            secondary
                .query_for_ids(OutputQuery::only_unspent())
                .await
                .len(),
            1
        );
    }

    /// overlooks a bunch of dust inputs to find a pair of inputs marginally big enough to send
    pub async fn ignore_dust_inputs<CC>()
    where
        CC: ConductChain,
    {
        let mut environment = CC::setup().await;

        let primary = environment.fund_client_orchard(120_000).await;
        let secondary = environment.create_client().await;

        // send a bunch of dust
        assert_eq!(
            with_assertions::propose_send_bump_sync_recipient(
                &mut environment,
                &primary,
                &secondary,
                vec![
                    (Shielded(Sapling), 1_000),
                    (Shielded(Sapling), 1_000),
                    (Shielded(Sapling), 1_000),
                    (Shielded(Sapling), 1_000),
                    (Shielded(Sapling), 15_000),
                    (Shielded(Orchard), 1_000),
                    (Shielded(Orchard), 1_000),
                    (Shielded(Orchard), 1_000),
                    (Shielded(Orchard), 1_000),
                    (Shielded(Orchard), 15_000),
                ],
            )
            .await,
            11 * MARGINAL_FEE.into_u64()
        );

        // combine the only valid sapling note with the only valid orchard note to send
        assert_eq!(
            with_assertions::propose_send_bump_sync_recipient(
                &mut environment,
                &secondary,
                &primary,
                vec![(Shielded(Orchard), 10_000),],
            )
            .await,
            4 * MARGINAL_FEE.into_u64()
        );
    }

    /// creates a proposal, sends it and receives it (upcoming: compares that it was executed correctly) in a chain-generic context
    pub async fn send_value_to_pool<CC>(send_value: u64, pooltype: PoolType)
    where
        CC: ConductChain,
    {
        let multiple = match pooltype {
            PoolType::Shielded(Orchard) => 2u64,
            PoolType::Shielded(Sapling) => 4u64,
            PoolType::Transparent => 3u64,
        };
        let mut environment = CC::setup().await;

        let sender = environment
            .fund_client_orchard(send_value + multiple * (MARGINAL_FEE.into_u64()))
            .await;

        let recipient = environment.create_client().await;
        let recipient_address = get_base_address(&recipient, pooltype).await;

        from_inputs::quick_send(
            &sender,
            vec![(recipient_address.as_str(), send_value, None)],
        )
        .await
        .unwrap();

        environment.bump_chain().await;

        recipient.do_sync(false).await.unwrap();

        assert_eq!(
            recipient
                .query_sum_value(OutputQuery {
                    spend_status: OutputSpendStatusQuery::only_unspent(),
                    pools: OutputPoolQuery::one_pool(pooltype),
                })
                .await,
            send_value
        );
    }

    /// In order to fund a transaction multiple notes may be selected and consumed.
    /// The algorithm selects the smallest covering note(s).
    pub async fn note_selection_order<CC>()
    where
        CC: ConductChain,
    {
        let number_of_notes = 4;

        let transaction_1_sends = (1..=number_of_notes).map(|n| n * 10_000);

        let expected_fee_for_transaction_1 = number_of_notes * 2 * MARGINAL_FEE.into_u64();
        // let expected_funds_from_transaction_1 = oeui.map(x);

        let mut environment = CC::setup().await;
        let primary = environment
            .fund_client_orchard(expected_fee_for_transaction_1)
            .await;
        let secondary = environment.create_client().await;

        // Send n=4 transfers in increasing 10_000 zat increments
        assert_eq!(
            with_assertions::propose_send_bump_sync_recipient(
                &mut environment,
                &primary,
                &secondary,
                transaction_1_sends
                    .map(|value| (Shielded(Sapling), value))
                    .collect()
            )
            .await,
            number_of_notes * 2 * MARGINAL_FEE.into_u64()
        );

        with_assertions::propose_send_bump_sync_recipient(
            &mut environment,
            &primary,
            &secondary,
            vec![(Shielded(Orchard), 40_000)],
        )
        .await;
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
            .collect::<Vec<_>>();
        // client_2 got a total of 3000+2000+1000
        // It sent 3000 to the client_1, and also
        // paid the default transaction fee.
        // In non change notes it has 1000.
        // There is an outstanding 2000 that is marked as change.
        // After sync the unspent_sapling_notes should go to 3000.
        assert_eq!(non_change_note_values.iter().sum::<u64>(), 10000u64);

        zingo_testutils::increase_height_and_wait_for_client(&regtest_manager, &recipient, 5)
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

        */
    }
}

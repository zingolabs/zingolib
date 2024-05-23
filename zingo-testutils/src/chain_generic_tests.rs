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
    use crate::check_client_balances;
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
                Transparent => 3,
                Shielded(Sapling) => 4, // but according to my reading of https://zips.z.cash/zip-0317, this should be 3
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

    /// sends back and forth several times, including sends to transparent
    pub async fn send_shield_cycle<CC>(n: u64)
    where
        CC: ConductChain,
    {
        let mut environment = CC::setup().await;
        let mut primary_fund = 1_000_000;
        let mut secondary_fund = 0u64;
        let primary = environment.fund_client_orchard(primary_fund).await;

        let secondary = environment.create_client().await;

        fn per_cycle_primary_debit(start: u64) -> u64 {
            start - 65_000u64
        }
        fn per_cycle_secondary_credit(start: u64) -> u64 {
            start + 25_000u64
        }
        for _ in 0..n {
            assert_eq!(
                with_assertions::propose_send_bump_sync_recipient(
                    &mut environment,
                    &primary,
                    &secondary,
                    vec![(Transparent, 100_000)],
                )
                .await,
                MARGINAL_FEE.into_u64() * 3
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

            primary_fund = per_cycle_primary_debit(primary_fund);
            secondary_fund = per_cycle_secondary_credit(secondary_fund);
            check_client_balances!(primary, o: primary_fund s: 0 t: 0);
            check_client_balances!(secondary, o: secondary_fund s: 0 t: 0);
        }
    }

    /// uses a dust input to pad another input to finish a transaction
    pub async fn send_grace_input<CC>()
    where
        CC: ConductChain,
    {
        let mut environment = CC::setup().await;
        let primary = environment.fund_client_orchard(110_000).await;

        let primary_address_orchard = get_base_address(&primary, Shielded(Orchard)).await;

        let secondary = environment.create_client().await;
        // let secondary_address_sapling = secondary.get_base_address(Shielded(Sapling)).await;
        let secondary_address_orchard = get_base_address(&secondary, Shielded(Orchard)).await;

        from_inputs::send(
            &primary,
            vec![
                (secondary_address_orchard.as_str(), 1, None),
                (secondary_address_orchard.as_str(), 99_999, None),
            ],
        )
        .await
        .unwrap();

        environment.bump_chain().await;
        secondary.do_sync(false).await.unwrap();

        check_client_balances!(secondary, o: 100_000 s: 0 t: 0);

        from_inputs::send(
            &secondary,
            vec![(primary_address_orchard.as_str(), 90_000, None)],
        )
        .await
        .unwrap();

        environment.bump_chain().await;
        primary.do_sync(false).await.unwrap();
        check_client_balances!(primary, o: 90_000 s: 0 t: 0);
    }

    /// overlooks a bunch of dust inputs to find a pair of inputs marginally big enough to send
    pub async fn ignore_dust_inputs<CC>()
    where
        CC: ConductChain,
    {
        let mut environment = CC::setup().await;
        let primary = environment.fund_client_orchard(120_000).await;

        let primary_address_orchard = get_base_address(&primary, Shielded(Orchard)).await;

        let secondary = environment.create_client().await;
        // let secondary_address_sapling = secondary.get_base_address(Shielded(Sapling)).await;
        let secondary_address_sapling = get_base_address(&secondary, Shielded(Sapling)).await;
        let secondary_address_orchard = get_base_address(&secondary, Shielded(Orchard)).await;

        from_inputs::send(
            &primary,
            vec![
                (secondary_address_sapling.as_str(), 1_000, None),
                (secondary_address_orchard.as_str(), 1_000, None),
                (secondary_address_sapling.as_str(), 1_000, None),
                (secondary_address_orchard.as_str(), 1_000, None),
                (secondary_address_sapling.as_str(), 1_000, None),
                (secondary_address_orchard.as_str(), 1_000, None),
                (secondary_address_sapling.as_str(), 1_000, None),
                (secondary_address_orchard.as_str(), 1_000, None),
                (secondary_address_sapling.as_str(), 1_000, None),
                (secondary_address_orchard.as_str(), 1_000, None),
                (secondary_address_sapling.as_str(), 10_000, None),
                (secondary_address_orchard.as_str(), 10_000, None),
            ],
        )
        .await
        .unwrap();

        environment.bump_chain().await;
        secondary.do_sync(false).await.unwrap();

        check_client_balances!(secondary, o: 15_000 s: 15_000 t: 0);

        from_inputs::send(
            &secondary,
            vec![(primary_address_orchard.as_str(), 5_001, None)],
        )
        .await
        .unwrap();

        environment.bump_chain().await;
        secondary.do_sync(false).await.unwrap();
        check_client_balances!(secondary, o: 9_999 s: 5_000 t: 0);
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
                    spend_status: OutputSpendStatusQuery {
                        unspent: true,
                        pending_spent: false,
                        spent: false,
                    },
                    pools: OutputPoolQuery::one_pool(pooltype),
                })
                .await,
            send_value
        );
    }
}

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

    use crate::lightclient::from_inputs;
    use crate::{assertions::assert_record_matches_step, lightclient::get_base_address};
    use crate::{chain_generic_tests::conduct_chain::ConductChain, check_client_balances};

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
                Shielded(Sapling) => 4,
                Shielded(Orchard) => 2,
            };

        let sender = environment
            .fund_client_orchard(send_value + expected_fee)
            .await;

        println!("client is ready to send");
        dbg!(sender.query_sum_value(OutputQuery::any()).await);
        dbg!(send_value);

        let recipient = environment.create_client().await;
        let recipient_address = crate::lightclient::get_base_address(&recipient, pooltype).await;
        let request = from_inputs::transaction_request_from_send_inputs(
            &recipient,
            vec![(recipient_address.as_str(), send_value, None)],
        )
        .unwrap();

        println!("recipient ready");

        let proposal = sender.propose_send(request).await.unwrap();
        let one_txid = sender
            .complete_and_broadcast_stored_proposal()
            .await
            .unwrap();
        let txid = one_txid.first();

        {
            let read_lock = sender
                .wallet
                .transaction_context
                .transaction_metadata_set
                .read()
                .await;

            let record = read_lock
                .transaction_records_by_id
                .get(txid)
                .expect("sender must recognize txid");

            let step = proposal.steps().first();

            assert_record_matches_step(record, step).await;
        }

        environment.bump_chain().await;

        sender.do_sync(false).await.unwrap();

        {
            let read_lock = sender
                .wallet
                .transaction_context
                .transaction_metadata_set
                .read()
                .await;

            let record = read_lock
                .transaction_records_by_id
                .get(txid)
                .expect("sender must recognize txid");

            let step = proposal.steps().first();

            assert_record_matches_step(record, step).await;
        }

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

    /// sends back and forth several times, including sends to transparent
    pub async fn send_shield_cycle<CC>(n: u64)
    where
        CC: ConductChain,
    {
        let mut environment = CC::setup().await;
        let mut primary_fund = 1_000_000 + (n + 6) * MARGINAL_FEE.into_u64();
        let mut secondary_fund = 0u64;
        let primary = environment.fund_client_orchard(primary_fund).await;
        let primary_address = get_base_address(&primary, Shielded(Orchard)).await;

        let secondary = environment.create_client().await;
        let secondary_address = crate::lightclient::get_base_address(&secondary, Transparent).await;

        fn per_cycle_primary_debit(start: u64) -> u64 {
            start - 65_000u64
        }
        fn per_cycle_secondary_credit(start: u64) -> u64 {
            start + 25_000u64
        }
        for _ in 0..n {
            from_inputs::quick_send(&primary, vec![(secondary_address.as_str(), 100_000, None)])
                .await
                .unwrap();
            environment.bump_chain().await;
            secondary.do_sync(false).await.unwrap();
            secondary.quick_shield().await.unwrap();
            environment.bump_chain().await;
            secondary.do_sync(false).await.unwrap();
            from_inputs::quick_send(&secondary, vec![(primary_address.as_str(), 50_000, None)])
                .await
                .unwrap();
            environment.bump_chain().await;
            primary.do_sync(false).await.unwrap();
            primary_fund = per_cycle_primary_debit(primary_fund);
            secondary_fund = per_cycle_secondary_credit(secondary_fund);
            check_client_balances!(primary, o: primary_fund s: 0 t: 0);
            check_client_balances!(secondary, o: secondary_fund s: 0 t: 0);
        }
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

        dbg!("chain set up, funding client now");

        let sender = environment
            .fund_client_orchard(send_value + multiple * (MARGINAL_FEE.into_u64()))
            .await;

        dbg!("client is ready to send");
        dbg!(sender.query_sum_value(OutputQuery::any()).await);
        dbg!(send_value);

        let recipient = environment.create_client().await;
        let recipient_address = crate::lightclient::get_base_address(&recipient, pooltype).await;

        dbg!("recipient ready");
        dbg!(recipient.query_sum_value(OutputQuery::any()).await);

        from_inputs::quick_send(
            &sender,
            vec![(dbg!(recipient_address).as_str(), send_value, None)],
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

//! tests that can be run either as lib-to-node or darkside.

//! this mod tests fair weather behavior i.e. the LightClient is connected to a server that provides expected responses about the state of the blockchain.
//! there are many ways to mock the chain. for simplicity, and in order to be usable in multiple contexts, the test fixtures in this mod delegate setup and server management to a ChainConductor (anything that implements ConductChain)
//

/// A Lightclient test may involve hosting a server to send data to the LightClient. This trait can be asked to set simple scenarios where a mock LightServer sends data showing a note to a LightClient, the LightClient updates and responds by sending the note, and the Lightserver accepts the transaction and rebroadcasts it...
/// The initial two implementors are
/// lib-to-node, which links a lightserver to a zcashd in regtest mode. see `impl ConductChain for LibtoNode
/// darkside, a mode for the lightserver which mocks zcashd. search 'impl ConductChain for DarksideScenario
pub mod conduct_chain {
    use zingolib::get_base_address;
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
        async fn fund_client(&mut self, value: u64) -> LightClient {
            let sender = self.create_faucet().await;
            let recipient = self.create_client().await;

            self.bump_chain().await;
            sender.do_sync(false).await.unwrap();

            sender
                .send_from_send_inputs(vec![(
                    (get_base_address!(recipient, "unified")).as_str(),
                    value,
                    None,
                )])
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

    /// runs a send-to-receiver and receives it in a chain-generic context
    pub async fn propose_and_broadcast_value_to_pool<TE>(send_value: u64, pooltype: PoolType)
    where
        TE: ConductChain,
    {
        let mut environment = TE::setup().await;

        println!("chain set up, funding client now");

        let expected_fee = MARGINAL_FEE.into_u64()
            * match pooltype {
                Transparent => 3,
                Shielded(Sapling) => 4,
                Shielded(Orchard) => 2,
            };

        let sender = environment.fund_client(send_value + expected_fee).await;

        println!("client is ready to send");
        dbg!(sender.query_sum_value(OutputQuery::any()).await);
        dbg!(send_value);

        let recipient = environment.create_client().await;
        let recipient_address = recipient.get_base_address(pooltype).await;
        let request = recipient
            .transaction_request_from_send_inputs(vec![(
                recipient_address.as_str(),
                send_value,
                None,
            )])
            .unwrap();

        println!("recipient ready");

        let proposal = sender.propose_send(request).await.unwrap();
        let one_txid = sender
            .complete_and_broadcast_stored_proposal()
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

    /// creates a proposal, sends it and receives it (upcoming: compares that it was executed correctly) in a chain-generic context
    pub async fn send_value_to_pool<TE>(send_value: u64, pooltype: PoolType)
    where
        TE: ConductChain,
    {
        let mut environment = TE::setup().await;

        dbg!("chain set up, funding client now");

        let sender = environment
            .fund_client(send_value + 2 * (MARGINAL_FEE.into_u64()))
            .await;

        dbg!("client is ready to send");
        dbg!(sender.query_sum_value(OutputQuery::any()).await);
        dbg!(send_value);

        let recipient = environment.create_client().await;
        let recipient_address = recipient.get_base_address(pooltype).await;

        dbg!("recipient ready");
        dbg!(recipient.query_sum_value(OutputQuery::any()).await);

        sender
            .send_from_send_inputs(vec![(dbg!(recipient_address).as_str(), send_value, None)])
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

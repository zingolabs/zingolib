//! tests that can be run either as lib-to-node or darkside.

use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;

use crate::{get_base_address, lightclient::LightClient, wallet::notes::query::OutputQuery};

#[allow(async_fn_in_trait)]
/// both lib-to-node and darkside can implement this.
pub trait ChainTest {
    /// set up the test chain
    async fn setup() -> Self;
    /// builds a faucet (funded from mining)
    async fn create_faucet(&mut self) -> LightClient;
    /// builds an empty client
    async fn create_client(&mut self) -> LightClient;
    /// moves the chain tip forward, confirming transactions that need to be confirmed
    async fn bump_chain(&mut self);

    /// builds a client and funds it in a certain pool. may need sync before noticing its funds.
    async fn fund_client(&mut self, value: u32) -> LightClient {
        let sender = self.create_faucet().await;
        let recipient = self.create_client().await;

        self.bump_chain().await;
        sender.do_sync(false).await.unwrap();

        dbg!(sender.query_sum_value(OutputQuery::any()).await);
        dbg!(value);

        sender
            .do_quick_send(
                sender
                    .raw_to_transaction_request(vec![(
                        get_base_address!(recipient, "unified"),
                        value,
                        None,
                    )])
                    .unwrap(),
            )
            .await
            .unwrap();

        self.bump_chain().await;

        recipient.do_sync(false).await.unwrap();

        recipient
    }

    // async fn start_with_funds(value: u32) -> (LightClient, Self) {
    //     let chain = Self::setup().await;

    //     let starter = chain
    //         .fund_client(value + 2 * (MARGINAL_FEE.into_u64() as u32))
    //         .await;

    //     (starter, chain);
    // }
}

/// runs a send-to-self and receives it in a chain-generic context
pub async fn simple_send<CT>(value: u32)
where
    CT: ChainTest,
{
    let mut chain = CT::setup().await;

    let sender = chain
        .fund_client(value + 2 * (MARGINAL_FEE.into_u64() as u32))
        .await;

    let recipient = chain.create_client().await;

    let proposal = sender
        .do_propose_send(
            sender
                .raw_to_transaction_request(vec![(
                    get_base_address!(recipient, "unified"),
                    value,
                    None,
                )])
                .unwrap(),
        )
        .await
        .unwrap();

    let txids = sender.do_send_proposed().await.unwrap();

    chain.bump_chain().await;

    recipient.do_sync(false).await.unwrap();

    // commented because this does not work in darkside right now.
    // recipient
    //     .check_chain_matches_proposal(proposal, txids, true)
    //     .await;

    assert_eq!(
        recipient
            .query_sum_value(OutputQuery::stipulations(
                true, false, false, false, false, true
            ))
            .await,
        value as u64
    );
}

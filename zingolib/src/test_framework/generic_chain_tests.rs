//! tests that can be run either as lib-to-node or darkside.

use zcash_client_backend::{PoolType, ShieldedProtocol::Orchard};

use crate::{get_base_address, lightclient::LightClient, wallet::notes::query::OutputQuery};

#[allow(async_fn_in_trait)]
/// both lib-to-node and darkside can implement this.
pub trait ChainTest {
    // /// set up the test chain
    // async fn setup() -> Self;
    /// builds a client and funds it in a certain pool. may need sync before noticing its funds.
    async fn build_client_and_fund(&self, funds: u32, pool: PoolType) -> LightClient;
    /// builds an empty client
    async fn build_client(&self) -> LightClient;
    /// moves the chain tip forward, confirming transactions that need to be confirmed
    async fn bump_chain(&self);
}

/// runs a send-to-self and receives it in a chain-generic context
pub async fn simple<CT>(chain: CT, value: u32)
where
    CT: ChainTest,
{
    let xsender = chain
        .build_client_and_fund(value * 2, PoolType::Shielded(Orchard))
        .await;
}

pub async fn send<CT>(chain: CT, value: u32)
where
    CT: ChainTest,
{
    let sender = chain
        .build_client_and_fund(value * 2, PoolType::Shielded(Orchard))
        .await;

    let recipient = chain.build_client().await;

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

    chain.bump_chain().await;

    recipient.do_sync(false).await.unwrap();

    let balance = recipient
        .query_sum_value(OutputQuery::stipulations(
            true, false, false, false, false, false,
        ))
        .await;

    assert_eq!(balance, value as u64);
}

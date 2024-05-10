//! tests that can be run either as lib-to-node or darkside.

use zcash_client_backend::PoolType;
use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;

use zingolib::lightclient::LightClient;
use zingolib::wallet::notes::query::OutputQuery;
use zingolib::wallet::notes::query::OutputSpendStatusQuery;
use zingolib::{get_base_address, wallet::notes::query::OutputPoolQuery};

#[allow(async_fn_in_trait)]
#[allow(opaque_hidden_inferred_bound)]
/// both lib-to-node and darkside can implement this.
/// implemented on LibtonodeChain and DarksideScenario respectively
pub trait ManageScenario {
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

        sender
            .do_send_test_only(vec![(
                (get_base_address!(recipient, "unified")).as_str(),
                value as u64,
                None,
            )])
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

/// creates a proposal, sends it and receives it (upcoming: compares that it was executed correctly) in a chain-generic context
pub async fn send_value_to_pool<TE>(send_value: u32, pooltype: PoolType)
where
    TE: ManageScenario,
{
    let mut environment = TE::setup().await;

    dbg!("chain set up, funding client now");

    let sender = environment
        .fund_client(send_value + 2 * (MARGINAL_FEE.into_u64() as u32))
        .await;

    dbg!("client is ready to send");
    dbg!(sender.query_sum_value(OutputQuery::any()).await);
    dbg!(send_value);

    let recipient = environment.create_client().await;
    let recipient_address = recipient.get_base_address(pooltype).await;

    dbg!("recipient ready");
    dbg!(recipient.query_sum_value(OutputQuery::any()).await);

    sender
        .do_send_test_only(vec![(
            dbg!(recipient_address).as_str(),
            send_value as u64,
            None,
        )])
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
        send_value as u64
    );
}

/// runs a send-to-receiver and receives it in a chain-generic context
pub async fn propose_and_broadcast_value_to_pool<TE>(send_value: u32, pooltype: PoolType)
where
    TE: ManageScenario,
{
    let mut environment = TE::setup().await;

    dbg!("chain set up, funding client now");

    let sender = environment
        .fund_client(send_value + 2 * (MARGINAL_FEE.into_u64() as u32))
        .await;

    dbg!("client is ready to send");
    dbg!(sender.query_sum_value(OutputQuery::any()).await);
    dbg!(send_value);

    let recipient = environment.create_client().await;
    let recipient_address = recipient.get_base_address(pooltype).await;
    let request = recipient
        .raw_to_transaction_request(vec![(dbg!(recipient_address), send_value, None)])
        .unwrap();

    dbg!("recipient ready");
    dbg!(recipient.query_sum_value(OutputQuery::any()).await);
    dbg!(request.clone());

    sender.propose_send(request).await.unwrap();
    sender
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
        send_value as u64
    );
}

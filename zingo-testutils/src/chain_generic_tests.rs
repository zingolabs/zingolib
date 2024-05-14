//! tests that can be run either as lib-to-node or darkside.

use zcash_client_backend::PoolType;
use zcash_client_backend::PoolType::Shielded;
use zcash_client_backend::PoolType::Transparent;
use zcash_client_backend::ShieldedProtocol::Orchard;
use zcash_client_backend::ShieldedProtocol::Sapling;
use zcash_primitives::transaction::fees::zip317::MARGINAL_FEE;

use zingolib::lightclient::LightClient;
use zingolib::wallet::notes::query::OutputPoolQuery;
use zingolib::wallet::notes::query::OutputQuery;
use zingolib::wallet::notes::query::OutputSpendStatusQuery;

#[allow(async_fn_in_trait)]
#[allow(opaque_hidden_inferred_bound)]
/// A Lightclient test may involve hosting a server to send data to the LightClient. This trait can be asked to set simple scenarios where a mock LightServer sends data showing a note to a LightClient, the LightClient updates and responds by sending the note, and the Lightserver accepts the transaction and rebroadcasts it...
/// The initial two implementors are
/// lib-to-node, which links a lightserver to a zcashd in regtest mode. see `impl ConductChain for LibtoNode
/// darkside, a mode for the lightserver which mocks zcashd. search 'impl ConductChain for DarksideScenario
pub trait ConductChain {
    /// set up the test chain
    async fn setup() -> Self;
    /// builds a faucet (funded from mining)
    async fn create_faucet(&mut self) -> LightClient;
    /// builds an empty client
    async fn create_client(&mut self) -> LightClient;
    /// moves the chain tip forward, confirming transactions that need to be confirmed
    async fn bump_chain(&mut self);

    /// builds a client and funds it in orchard and syncs it
    async fn fund_client(&mut self, value: u64) -> LightClient {
        let sender = self.create_faucet().await;
        let recipient = self.create_client().await;

        self.bump_chain().await;
        sender.do_sync(false).await.unwrap();

        let recipient_address = recipient.get_base_address(Shielded(Orchard)).await;
        let request = recipient
            .raw_to_transaction_request(vec![(recipient_address, value, None)])
            .unwrap();
        let _one_txid = sender.quick_send(request).await.unwrap();

        self.bump_chain().await;

        recipient.do_sync(false).await.unwrap();

        recipient
    }

    // async fn start_with_funds(value: u64) -> (LightClient, Self) {
    //     let chain = Self::setup().await;

    //     let starter = chain
    //         .fund_client(value + 2 * (MARGINAL_FEE.into_u64() as u64))
    //         .await;

    //     (starter, chain);
    // }
}

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
        .raw_to_transaction_request(vec![(dbg!(recipient_address), send_value, None)])
        .unwrap();

    println!("recipient ready");

    let proposal = sender.propose_send(request).await.unwrap();
    assert_eq!(proposal.steps().len(), 1);
    assert_eq!(
        proposal.steps().first().balance().fee_required().into_u64(),
        expected_fee
    );

    let one_txid = sender
        .complete_and_broadcast_stored_proposal()
        .await
        .unwrap();

    let txid = one_txid.first();
    let read_lock = sender
        .wallet
        .transaction_context
        .transaction_metadata_set
        .read()
        .await;
    let transaction_record = read_lock
        .transaction_records_by_id
        .get(txid)
        .expect("sender must recognize txid");

    assert!(!transaction_record.status.is_confirmed());

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
    let request = recipient
        .raw_to_transaction_request(vec![(dbg!(recipient_address), send_value, None)])
        .unwrap();

    println!("recipient ready");

    let _one_txid = sender.quick_send(request).await.unwrap();

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

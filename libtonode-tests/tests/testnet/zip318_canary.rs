use zcash_protocol::ShieldedPool;
use zingolib::testutils::chain_generics::conduct_chain::ConductChain;
use zingolib::testutils::chain_generics::networked::NetworkedTestEnvironment;
use zingolib::testutils::lightclient::from_inputs;
use zingolib::wallet::keys::unified::ReceiverSelection;
use zip32::AccountId;

const PAYMENT: u64 = 20_000;
/// The spec target of ZIP 318 on a chain past NU6.3: a payment to an Orchard
/// receiver lands in the Ironwood pool. When the network's verdict moves, this
/// cell fails and the expectation and ADR 0009 are flipped together.
const EXPECTED_RECEIVING_POOL: ShieldedPool = ShieldedPool::Ironwood;

/// - Awaits real testnet blocks, syncing after each, until the payment confirms or the patience runs out.
#[tokio::test]
async fn a_payment_to_an_orchard_receiver_lands_in_the_expected_pool() {
    let mut environment = NetworkedTestEnvironment::setup().await;
    let mut client = environment.create_faucet().await;
    let network = client.chain_type();
    let (_, orchard_only) = client
        .generate_unified_address(ReceiverSelection::orchard_only(), AccountId::ZERO)
        .await
        .expect("the wallet derives an orchard-only address");
    let orchard_only = orchard_only.encode(&network);

    let txid = *from_inputs::quick_send(&mut client, vec![(&orchard_only, PAYMENT, None)])
        .await
        .expect("the network accepts the payment to the Orchard receiver")
        .first();

    for _ in 0..environment.confirmation_patience_blocks() {
        environment.increase_chain_height().await;
        environment.sync_client_to_tip(&mut client).await;
        let confirmed = client
            .wallet()
            .read()
            .await
            .wallet_transactions
            .get(&txid)
            .is_some_and(|transaction| transaction.status().is_confirmed());
        if confirmed {
            break;
        }
    }

    let summary = client
        .transaction_summaries(false)
        .await
        .expect("a synced wallet summarizes its transactions")
        .0
        .into_iter()
        .find(|summary| summary.txid == txid)
        .expect("the payment is recorded");
    assert!(
        summary.status.is_confirmed(),
        "the network must confirm the payment within the patience"
    );
    let (landed_in_ironwood, landed_in_orchard) = (
        summary
            .ironwood_notes
            .iter()
            .any(|note| note.value == PAYMENT),
        summary
            .orchard_notes
            .iter()
            .any(|note| note.value == PAYMENT),
    );
    let landed_in = match (landed_in_ironwood, landed_in_orchard) {
        (true, false) => ShieldedPool::Ironwood,
        (false, true) => ShieldedPool::Orchard,
        other => panic!("the payment must land in exactly one of the two pools, found {other:?}"),
    };
    assert_eq!(
        landed_in, EXPECTED_RECEIVING_POOL,
        "the network's verdict on a payment to an Orchard receiver has moved: flip this \
         expectation and amend ADR 0009 together"
    );
}

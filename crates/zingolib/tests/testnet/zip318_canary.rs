use zcash_protocol::PoolType;
use zingolib::testutils::chain_generics::conduct_chain::ConductChain;
use zingolib::testutils::chain_generics::networked::NetworkedTestEnvironment;
use zingolib::testutils::lightclient::from_inputs;
use zingolib::wallet::keys::unified::ReceiverSelection;
use zingolib::wallet::propose::payment_pool;
use zip32::AccountId;

use super::SELF_SEND_VALUE;

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
    let chain_height = client
        .wallet()
        .read()
        .await
        .sync_state
        .last_known_chain_height()
        .expect("the synced faucet knows the chain height");
    // The wallet's own routing rule names the pool the spec target sends to at
    // this height; the network's only verdict is whether it accepts the bytes.
    let expected_pool = payment_pool(&network, chain_height);

    let txid = *from_inputs::quick_send(&mut client, vec![(&orchard_only, SELF_SEND_VALUE, None)])
        .await
        .expect("the network accepts the payment to the Orchard receiver")
        .first();
    assert!(
        environment.await_confirmed(&mut client, &[txid]).await,
        "the network must confirm the payment within the patience"
    );

    let summary = client
        .transaction_summaries(false)
        .await
        .expect("a synced wallet summarizes its transactions")
        .0
        .into_iter()
        .find(|summary| summary.txid == txid)
        .expect("the payment is recorded");
    assert_eq!(
        summary.pools_received(),
        vec![PoolType::Shielded(expected_pool)],
        "the payment and its change must land in the pool the wallet routes to at this \
         height; a different landing means the network's verdict moved, so flip ADR 0009"
    );
}

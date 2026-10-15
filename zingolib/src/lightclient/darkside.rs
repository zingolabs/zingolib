//! Reorg-semantics protections migrated from darkside-tests onto the
//! stateful mock indexer (user direction, 2026-07-08).
//!
//! The darkside-tests crate froze on two darkside-mode bugs in the
//! legacy indexer ("invalid block hash length in tree states",
//! post-reorg prev-hash continuity), leaving all nine of its tests
//! ignored. The protections they held (a wallet's behavior when the
//! chain changes its mind) port here onto
//! [`crate::testutils::mock_indexer`], whose [`MockChain::reorg_to`]
//! primitive plays the darksidewalletd role without the legacy stack
//! or either of its bugs.
//!
//! Ported: receipt removed by reorg (`reorg_receipt_sync_generic`),
//! receipt moved to a new height (`reorg_changes_incoming_tx_height`),
//! receipt reorged away and never re-mined
//! (`reorg_expires_incoming_tx`), and an outgoing send reorged out
//! until it expires (`sent_transaction_reorged_into_mempool`,
//! `reorg_expires_outgoing_tx_height`). Not yet ported: the
//! transaction-index variants (`reorg_changes_{incoming,outgoing}_tx_index`,
//! which need index-position assertions the summary surface does not
//! yet expose), `reorg_changes_outgoing_tx_height`'s re-mine leg
//! (subsumed here by the incoming variant plus the expiry legs), and
//! the two proptest `any_source_sends_to_any_receiver` fuzzers
//! (formerly blocked on the pepper-sync `darkside_test` feature
//! question, zingolabs/zingolib#2447. That feature is now deleted).

use zcash_protocol::PoolType;
use zcash_protocol::ShieldedPool;
use zcash_protocol::consensus::BlockHeight;
use zingo_status::confirmation_status::ConfirmationStatus;

use crate::check_client_balances;
use crate::testutils::lightclient::{from_inputs, get_base_address};
use crate::testutils::mock_indexer::{MockNet, faucet_funding_transaction};

const FUNDING: u64 = 100_000_000;

/// Fund `net`'s next block-but-two with a 100_000_000-zat ironwood note
/// to `address` (the faucet's V6 send routes the payment through the
/// ironwood bundle): two empty blocks, the funding block at height 3,
/// two more empties to give it confirmations. Mirrors the
/// darksidewalletd preparation the originals shared.
async fn fund_at_height_three(net: &MockNet, address: &str) -> Vec<u8> {
    let funding = faucet_funding_transaction(vec![(address, FUNDING, None)]).await;
    let mut chain = net.chain.write().await;
    chain.mine_empty_blocks(2);
    chain.mine_block(vec![funding.clone()]);
    chain.mine_empty_blocks(2);
    funding
}

/// A reorg deep enough to orphan the funding receipt, onto a branch
/// that never re-mines it: the receipt and its balance must vanish.
/// Port of `reorg_receipt_sync_generic` and the assertion core of
/// `reorg_expires_incoming_tx`.
#[tokio::test]
async fn reorg_removes_receipt() {
    let mut net = MockNet::launch().await;
    let mut wallet = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED, None)
        .await;
    let address = get_base_address(&wallet, PoolType::Shielded(ShieldedPool::Orchard)).await;
    fund_at_height_three(&net, &address).await;

    wallet.sync_and_await().await.unwrap();
    check_client_balances!(wallet, i: FUNDING o: 0 s: 0 t: 0);

    // The new branch is longer and funding-free.
    {
        let mut chain = net.chain.write().await;
        let evicted = chain.reorg_to(2);
        assert_eq!(evicted.len(), 1, "only the funding tx is evicted");
        chain.mine_empty_blocks(5);
    }
    wallet.sync_and_await().await.unwrap();
    check_client_balances!(wallet, i: 0 o: 0 s: 0 t: 0);
}

/// A reorg that re-mines the funding transaction at a different
/// height: the balance survives and the receipt's confirmation height
/// follows the chain. Port of `reorg_changes_incoming_tx_height`.
#[tokio::test]
async fn reorg_moves_receipt_to_new_height() {
    let mut net = MockNet::launch().await;
    let mut wallet = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED, None)
        .await;
    let address = get_base_address(&wallet, PoolType::Shielded(ShieldedPool::Orchard)).await;
    fund_at_height_three(&net, &address).await;

    wallet.sync_and_await().await.unwrap();
    let confirmed_at = |summaries: &crate::wallet::summary::data::TransactionSummaries| {
        summaries
            .iter()
            .find(|summary| summary.value == FUNDING)
            .expect("the funding receipt has a summary")
            .status
    };
    let summaries = wallet.transaction_summaries(false).await.unwrap();
    assert_eq!(
        confirmed_at(&summaries),
        ConfirmationStatus::Confirmed(BlockHeight::from_u32(3)),
    );

    // Reorg out the funding block and re-mine the same transaction two
    // heights later on the new branch.
    {
        let mut chain = net.chain.write().await;
        let evicted = chain.reorg_to(2);
        assert_eq!(evicted.len(), 1);
        chain.mine_empty_blocks(2);
        chain.mine_block(evicted);
        chain.mine_empty_blocks(2);
    }
    wallet.sync_and_await().await.unwrap();
    check_client_balances!(wallet, i: FUNDING o: 0 s: 0 t: 0);
    let summaries = wallet.transaction_summaries(false).await.unwrap();
    assert_eq!(
        confirmed_at(&summaries),
        ConfirmationStatus::Confirmed(BlockHeight::from_u32(5)),
        "the receipt's confirmation height follows the re-mined block"
    );
}

/// An outgoing spend whose block is reorged away and never re-mined:
/// once the transaction's expiry height passes on the new branch, the
/// spend unwinds and the sender's full balance returns. Port of
/// `sent_transaction_reorged_into_mempool` (whose original asserted
/// exactly this restored balance) and `reorg_expires_outgoing_tx_height`.
#[tokio::test]
async fn reorg_expires_outgoing_transaction() {
    let mut net = MockNet::launch().await;
    let mut sender = net
        .client(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED, None)
        .await;
    // Not ABANDON_ART: that is the synthetic funding faucet's own seed,
    // so its change output would land in this wallet and pollute the
    // recipient's balance assertions.
    let mut recipient = net
        .client(zingo_test_vectors::seeds::DARKSIDE_SEED, None)
        .await;
    let sender_address = get_base_address(&sender, PoolType::Shielded(ShieldedPool::Orchard)).await;
    let recipient_address =
        get_base_address(&recipient, PoolType::Shielded(ShieldedPool::Orchard)).await;
    fund_at_height_three(&net, &sender_address).await;
    sender.sync_and_await().await.unwrap();
    check_client_balances!(sender, i: FUNDING o: 0 s: 0 t: 0);

    // The send lands in the mock mempool; mine it at height 6.
    from_inputs::quick_send(&mut sender, vec![(&recipient_address, 10_000, None)])
        .await
        .unwrap();
    {
        let mut chain = net.chain.write().await;
        chain.mine_mempool();
        chain.mine_empty_blocks(1);
    }
    sender.sync_and_await().await.unwrap();
    recipient.sync_and_await().await.unwrap();
    let spent_less_fee = FUNDING - 10_000 - 10_000;
    check_client_balances!(sender, i: spent_less_fee o: 0 s: 0 t: 0);
    check_client_balances!(recipient, i: 10_000 o: 0 s: 0 t: 0);

    // Reorg the send's block away and never re-mine it; the branch
    // grows far past the transaction's expiry height.
    {
        let mut chain = net.chain.write().await;
        let evicted = chain.reorg_to(5);
        assert_eq!(evicted.len(), 1, "only the send is evicted");
        chain.mine_empty_blocks(120);
    }
    sender.sync_and_await().await.unwrap();
    recipient.sync_and_await().await.unwrap();
    check_client_balances!(sender, i: FUNDING o: 0 s: 0 t: 0);
    check_client_balances!(recipient, i: 0 o: 0 s: 0 t: 0);
}

use zcash_primitives::transaction::TxId;

use super::{continuous_sync_wallet_settings, external_address, fund};
use crate::perspective::value_transfer::{
    SelfSendValueTransfer, SentValueTransfer, ValueTransferKind, ValueTransfers,
};
use crate::testutils::mock_indexer::{MockNet, faucet_funding_transaction_on};

mod bound_note_reservation;
mod faucet_recipient_default_in_one_chain;
mod immediate_migration;
mod mempool_spend_accounting;
mod received_mempool_status;
mod rescan_self_send;
mod rescan_value_transfers;
mod send_pre_ironwood;
mod sync_all_expressible_epochs;
mod two_phase_migration;
mod unified_address_discovery;
mod untracked_history;
mod watch_only;

const HOSPITAL_MUSEUM: &str = zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;
const FUNDING: u64 = 100_000;
const DEFERRED_NU6_3: u32 = 130;

/// - Mines one block carrying a funding transaction built for the chain's own era at its next height.
async fn fund_on_chain(net: &MockNet, receivers: Vec<(&str, u64, Option<&str>)>) {
    let (activation_heights, height) = {
        let chain = net.chain.read().await;
        (chain.activation_heights(), chain.next_height())
    };
    let funding = faucet_funding_transaction_on(activation_heights, height, receivers).await;
    net.chain.write().await.mine_block(vec![funding]);
}

/// - Panics when a txid of `txids` has no migration value transfer.
fn migrated_value(value_transfers: &ValueTransfers, txids: &[TxId]) -> u64 {
    txids
        .iter()
        .map(|txid| {
            value_transfers
                .iter()
                .find(|vt| {
                    vt.txid == *txid
                        && vt.kind
                            == ValueTransferKind::Sent(SentValueTransfer::SendToSelf(
                                SelfSendValueTransfer::Migration,
                            ))
                })
                .unwrap_or_else(|| {
                    panic!("{txid:?} must be classified as a migration value transfer")
                })
                .value
        })
        .sum()
}

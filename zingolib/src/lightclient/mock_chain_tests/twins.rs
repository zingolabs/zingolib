use zcash_primitives::transaction::TxId;

use super::{
    DEFERRED_NU6_3, FUNDING, HOSPITAL_MUSEUM, confirmed_balance, continuous_sync_wallet_settings,
    external_address, external_transparent_address, fund, funded_client,
};
use crate::perspective::value_transfer::{
    SelfSendValueTransfer, SentValueTransfer, ValueTransferKind, ValueTransfers,
};

mod bound_note_reservation;
mod faucet_recipient_default_in_one_chain;
mod immediate_migration;
mod mempool_spend_accounting;
mod moved_from_regtest;
mod received_mempool_status;
mod rescan_self_send;
mod rescan_value_transfers;
mod send_pre_ironwood;
mod sync_all_expressible_epochs;
mod two_phase_migration;
mod unified_address_discovery;
mod untracked_history;
mod watch_only;

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

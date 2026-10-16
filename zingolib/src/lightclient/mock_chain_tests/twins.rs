use zcash_primitives::transaction::TxId;
use zcash_protocol::value::Zatoshis;

use super::{
    DEFERRED_NU6_3, FUNDING, HOSPITAL_MUSEUM, confirmed_balance, continuous_sync_wallet_settings,
    external_address, external_transparent_address, fund, funded_client, funding_at_next_height,
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

fn zatoshis(value: u64) -> Zatoshis {
    Zatoshis::from_u64(value).expect("test values stay within the money range")
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

/// Where a wallet's rescan stands against the chain tip after one more sync session.
#[derive(Debug, PartialEq, Eq)]
enum RescanProgress {
    CaughtUp,
    Behind,
    Exhausted,
}

fn rescan_progress(
    scanned: Option<u32>,
    tip: u32,
    sessions: usize,
    limit: usize,
) -> RescanProgress {
    unimplemented!(
        "a wallet scanned to {scanned:?} against tip {tip} after {sessions} of {limit} sessions"
    )
}

#[test]
fn a_wallet_scanned_to_the_tip_or_past_it_has_caught_up_whatever_the_session_count() {
    for sessions in [0, 1, 5] {
        assert_eq!(
            rescan_progress(Some(10), 10, sessions, 5),
            RescanProgress::CaughtUp
        );
        assert_eq!(
            rescan_progress(Some(11), 10, sessions, 5),
            RescanProgress::CaughtUp
        );
    }
}

#[test]
fn a_wallet_below_the_tip_is_behind_until_the_sessions_run_out() {
    assert_eq!(rescan_progress(Some(9), 10, 1, 5), RescanProgress::Behind);
    assert_eq!(rescan_progress(None, 10, 4, 5), RescanProgress::Behind);
    assert_eq!(
        rescan_progress(Some(9), 10, 5, 5),
        RescanProgress::Exhausted
    );
    assert_eq!(rescan_progress(None, 10, 6, 5), RescanProgress::Exhausted);
}

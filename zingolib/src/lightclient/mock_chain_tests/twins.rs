use zcash_primitives::transaction::TxId;
use zcash_primitives::transaction::fees::zip317::{GRACE_ACTIONS, MARGINAL_FEE, MINIMUM_FEE};
use zcash_protocol::value::Zatoshis;

use super::{
    DEFERRED_NU6_3, FUNDING, HOSPITAL_MUSEUM, confirmed_balance, continuous_sync_wallet_settings,
    external_address, external_transparent_address, fund, fund_client, funded_client,
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

fn blocks_past_activation(tip: u32, activation: u32) -> Option<u32> {
    (tip < activation).then(|| activation - tip + 1)
}

fn zip317_fee(logical_actions: u64) -> u64 {
    let grace = u64::try_from(GRACE_ACTIONS).expect("the grace action count is a small constant");
    u64::from(MARGINAL_FEE) * logical_actions.max(grace)
}

#[test]
fn the_runway_lands_the_tip_one_block_past_the_activation() {
    for (tip, activation) in [(2, 130), (101, 200), (5, 6)] {
        let runway = blocks_past_activation(tip, activation)
            .expect("each tip in the table sits below its activation");
        assert_eq!(
            tip + runway,
            activation + 1,
            "tip {tip}, activation {activation}"
        );
    }
}

#[test]
fn a_tip_at_or_past_the_activation_has_no_runway() {
    assert_eq!(blocks_past_activation(130, 130), None);
    assert_eq!(blocks_past_activation(131, 130), None);
}

#[test]
fn the_zip317_fee_floors_at_the_grace_actions() {
    let grace = u64::try_from(GRACE_ACTIONS).expect("the grace action count is a small constant");
    for logical_actions in 0..=grace {
        assert_eq!(zip317_fee(logical_actions), u64::from(MINIMUM_FEE));
    }
}

#[test]
fn the_zip317_fee_grows_by_one_marginal_fee_per_action_past_the_grace() {
    let grace = u64::try_from(GRACE_ACTIONS).expect("the grace action count is a small constant");
    for extra in 1..=4 {
        assert_eq!(
            zip317_fee(grace + extra),
            u64::from(MINIMUM_FEE) + extra * u64::from(MARGINAL_FEE)
        );
    }
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
    match scanned {
        Some(height) if height >= tip => RescanProgress::CaughtUp,
        _ if sessions < limit => RescanProgress::Behind,
        _ => RescanProgress::Exhausted,
    }
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

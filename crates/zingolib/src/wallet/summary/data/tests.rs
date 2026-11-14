use super::{BasicNoteSummary, SendType, TransactionKind, TransactionSummary};
use crate::wallet::output::SpendStatus;
use zcash_protocol::{PoolType, TxId};
use zl_common::status::ConfirmationStatus;

fn note(value: u64) -> BasicNoteSummary {
    BasicNoteSummary::from_parts(value, SpendStatus::Unspent, 0, None)
}

/// A minimal send-to-self summary with the given funding pools and received
/// Ironwood/Orchard notes. Every other field is empty or a neutral default.
fn self_send_summary(
    pools_sent_from: Vec<PoolType>,
    ironwood_notes: Vec<BasicNoteSummary>,
    orchard_notes: Vec<BasicNoteSummary>,
) -> TransactionSummary {
    TransactionSummary {
        txid: TxId::from_bytes([0; 32]),
        datetime: 0,
        status: ConfirmationStatus::Confirmed(10u32.into()),
        blockheight: 10u32.into(),
        kind: TransactionKind::Sent(SendType::SendToSelf),
        value: 0,
        fee: Some(0),
        zec_price: None,
        pools_sent_from,
        ironwood_notes,
        orchard_notes,
        sapling_notes: vec![],
        transparent_coins: vec![],
        outgoing_ironwood_notes: vec![],
        outgoing_orchard_notes: vec![],
        outgoing_sapling_notes: vec![],
        outgoing_transparent_coins: vec![],
        op_return_data: vec![],
    }
}

#[test]
fn orchard_funded_ironwood_receive_is_a_migration() {
    // The shape of a migration part: Orchard notes spent, value landing in
    // the wallet's own Ironwood pool.
    let summary = self_send_summary(vec![PoolType::ORCHARD], vec![note(100_000)], vec![]);
    assert!(summary.is_orchard_to_ironwood_migration());
}

#[test]
fn orchard_to_orchard_note_split_is_not_a_migration() {
    // A note-splitting round funds from Orchard and receives Orchard change,
    // with nothing landing in the Ironwood pool.
    let summary = self_send_summary(vec![PoolType::ORCHARD], vec![], vec![note(100_000)]);
    assert!(!summary.is_orchard_to_ironwood_migration());
}

#[test]
fn ironwood_receive_without_orchard_funding_is_not_a_migration() {
    // Value arriving in Ironwood but not funded from Orchard is not a
    // migration, whether the funding side is empty or another pool.
    let unfunded = self_send_summary(vec![], vec![note(100_000)], vec![]);
    assert!(!unfunded.is_orchard_to_ironwood_migration());

    let ironwood_funded = self_send_summary(vec![PoolType::IRONWOOD], vec![note(100_000)], vec![]);
    assert!(!ironwood_funded.is_orchard_to_ironwood_migration());
}

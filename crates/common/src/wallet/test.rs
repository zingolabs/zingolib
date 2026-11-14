use zcash_protocol::TxId;
use zcash_protocol::consensus::BlockHeight;

use super::WalletTransaction;
use crate::status::ConfirmationStatus;

fn transaction_with_status(status: ConfirmationStatus) -> WalletTransaction {
    WalletTransaction::new_for_test(TxId::from_bytes([0; 32]), status)
}

#[test]
fn update_status_follows_transaction_lifecycle() {
    let mut transaction =
        transaction_with_status(ConfirmationStatus::Calculated(BlockHeight::from_u32(10)));

    transaction.update_status(
        ConfirmationStatus::Transmitted(BlockHeight::from_u32(10)),
        1,
        false,
    );
    assert!(matches!(
        transaction.status(),
        ConfirmationStatus::Transmitted(_)
    ));

    transaction.update_status(
        ConfirmationStatus::Mempool(BlockHeight::from_u32(11)),
        2,
        false,
    );
    assert!(matches!(
        transaction.status(),
        ConfirmationStatus::Mempool(_)
    ));

    transaction.update_status(
        ConfirmationStatus::Confirmed(BlockHeight::from_u32(11)),
        3,
        false,
    );
    assert!(matches!(
        transaction.status(),
        ConfirmationStatus::Confirmed(_)
    ));
}

#[test]
fn update_status_failed_returns_to_mempool() {
    let mut transaction =
        transaction_with_status(ConfirmationStatus::Failed(BlockHeight::from_u32(10)));

    transaction.update_status(
        ConfirmationStatus::Mempool(BlockHeight::from_u32(11)),
        1,
        false,
    );
    assert!(matches!(
        transaction.status(),
        ConfirmationStatus::Mempool(_)
    ));
}

#[test]
fn update_status_failed_is_not_directly_confirmable() {
    // confirming a failed transaction is handled by replacing the whole record during block
    // scanning, not by a status update.
    let mut transaction =
        transaction_with_status(ConfirmationStatus::Failed(BlockHeight::from_u32(10)));

    transaction.update_status(
        ConfirmationStatus::Confirmed(BlockHeight::from_u32(11)),
        1,
        false,
    );
    assert!(matches!(
        transaction.status(),
        ConfirmationStatus::Failed(_)
    ));
}

#[test]
fn update_status_confirmed_is_terminal() {
    let mut transaction =
        transaction_with_status(ConfirmationStatus::Confirmed(BlockHeight::from_u32(10)));

    transaction.update_status(
        ConfirmationStatus::Mempool(BlockHeight::from_u32(11)),
        1,
        false,
    );
    assert!(matches!(
        transaction.status(),
        ConfirmationStatus::Confirmed(_)
    ));

    transaction.update_status(
        ConfirmationStatus::Failed(BlockHeight::from_u32(11)),
        2,
        false,
    );
    assert!(matches!(
        transaction.status(),
        ConfirmationStatus::Confirmed(_)
    ));
}

#[test]
fn update_status_forces_confirmed_to_fail() {
    let mut transaction =
        transaction_with_status(ConfirmationStatus::Confirmed(BlockHeight::from_u32(10)));

    transaction.update_status(
        ConfirmationStatus::Failed(BlockHeight::from_u32(11)),
        2,
        true,
    );
    assert!(matches!(
        transaction.status(),
        ConfirmationStatus::Failed(_)
    ));
}

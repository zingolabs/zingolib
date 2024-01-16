use crate::{BlockHeight, TransactionRecord, TransparentNote, TxId};
// Other necessary imports...

pub fn create_mock_transaction_record(confirmed: bool) -> TransactionRecord {
    let txid = TxId::from_bytes([0u8; 32]);
    let spent_txid = TxId::from_bytes([1u8; 32]);

    // Construct TransactionRecord with either confirmed or unconfirmed status
    let mut mock_record = TransactionRecord::new(
        if confirmed {
            zingo_status::confirmation_status::ConfirmationStatus::Confirmed(BlockHeight::from_u32(
                5,
            ))
        } else {
            zingo_status::confirmation_status::ConfirmationStatus::Unconfirmed
        },
        // other parameters...
    );

    // Add any additional data to mock_record if necessary

    mock_record
}

// This function can be used in your test cases and other parts of the codebase.

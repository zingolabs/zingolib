pub(crate) mod macros;
mod mocks;

#[allow(dead_code)]
pub(crate) fn create_transaction_record_with_one_tnote(
) -> crate::wallet::transaction_record::TransactionRecord {
    // A single transparent note makes is_incoming_trsaction true.
    let txid = zcash_primitives::transaction::TxId::from_bytes([0u8; 32]);
    let transparent_note = mocks::TransparentNoteBuilder::new()
        .address("t".to_string())
        .spent(Some((txid, 3)))
        .build();
    let mut transaction_record = crate::wallet::transaction_record::TransactionRecord::new(
        zingo_status::confirmation_status::ConfirmationStatus::Confirmed(
            zcash_primitives::consensus::BlockHeight::from_u32(5),
        ),
        1705077003,
        &txid,
    );
    transaction_record.transparent_notes.push(transparent_note);
    transaction_record
}

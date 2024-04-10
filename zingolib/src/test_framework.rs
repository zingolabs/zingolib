use zcash_primitives::transaction::TxId;

use crate::wallet::notes::TransparentNote;
pub(crate) mod macros;
mod mocks;

#[allow(dead_code)]
pub(crate) fn create_txid_and_tnote() -> (zcash_primitives::transaction::TxId, TransparentNote) {
    // A single transparent note makes is_incoming_trsaction true.
    let txid = zcash_primitives::transaction::TxId::from_bytes([0u8; 32]);
    (
        txid,
        mocks::TransparentNoteBuilder::new()
            .address("t".to_string())
            .spent(Some((txid, 3)))
            .build(),
    )
}
#[allow(dead_code)]
pub(crate) fn create_transaction_record_with_one_tnote(
    txid: TxId,
    transparent_note: TransparentNote,
) -> crate::wallet::transaction_record::TransactionRecord {
    // A single transparent note makes is_incoming_trsaction true.
    let (txid, transparent_note) = create_tnote();
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
#[allow(dead_code)]
pub(crate) fn create_note_record_id() -> crate::wallet::notes::NoteRecordIdentifier {
    let (txid, tnote) = create_txid_and_tnote();
    let transaction_record = create_transaction_record_with_one_tnote(txid, tnote);
    let index = 5u32;
    let txid = transaction_record.txid;
    crate::wallet::notes::NoteRecordIdentifier {
        txid,
        pool: zcash_client_backend::PoolType::Transparent,
        index,
    }
}

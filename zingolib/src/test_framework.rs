use zcash_primitives::transaction::TxId;

use crate::wallet::notes::TransparentNote;
pub(crate) mod macros;
pub mod mocks;

#[allow(dead_code)]
pub(crate) fn create_note_record_id() -> crate::wallet::notes::NoteRecordIdentifier {
    let transparent_note = TransparentNote::mock();
    let index = 5u32;
    crate::wallet::notes::NoteRecordIdentifier {
        txid: transparent_note.txid,
        pool: zcash_client_backend::PoolType::Transparent,
        index,
    }
}

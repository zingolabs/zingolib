//! contains functions that compare structs to see if they match

use zcash_client_backend::proposal::Step;
use zcash_client_backend::wallet::NoteId;

use zingolib::wallet::transaction_record::TransactionRecord;
use zingolib::wallet::transaction_records_by_id::TransactionRecordsById;

/// does this record match this step?
/// currently only checks if the fee matches
/// may fail in uncertain ways if used on a transaction we dont have an OutgoingViewingKey for
/// this currently fails for any broadcast but not confirmed transaction: it seems like get_transaction_fee does not recognize pending spends
pub async fn assert_record_matches_step(
    records: &TransactionRecordsById,
    record: &TransactionRecord,
    step: &Step<NoteId>,
) {
    let balance = step.balance();
    assert_eq!(
        records.calculate_transaction_fee(record).unwrap(),
        balance.fee_required().into_u64()
    );
    // there is more to be done to fully match these objects
}

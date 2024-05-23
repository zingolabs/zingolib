//! contains functions that compare structs to see if they match

use nonempty::NonEmpty;
use zcash_client_backend::proposal::Step;

use zcash_client_backend::wallet::NoteId;

use zcash_primitives::transaction::TxId;
use zingolib::data::proposal::TransferProposal;
use zingolib::lightclient::LightClient;
use zingolib::wallet::transaction_record::TransactionRecord;

/// assert send outputs match client
pub async fn assert_send_outputs_match_client(
    client: LightClient,
    proposal: TransferProposal,
    txids: NonEmpty<TxId>,
) {
}

/// does this record match this step?
/// currently only checks if the fee matches
/// may fail in uncertain ways if used on a transaction we dont have an OutgoingViewingKey for
/// this currently fails for any broadcast but not confirmed transaction: it seems like get_transaction_fee does not recognize pending spends
pub async fn assert_record_matches_step(record: &TransactionRecord, step: &Step<NoteId>) {
    let balance = step.balance();
    assert_eq!(
        record.get_transaction_fee().unwrap(),
        balance.fee_required().into_u64()
    );
    // there is more to be done to fully match these objects
}

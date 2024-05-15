//! contains functions that compare structs to see if they match

use zcash_client_backend::proposal::Proposal;
use zcash_primitives::transaction::TxId;

use zingolib::lightclient::LightClient;

/// call this after calling complete_and_broadcast_proposal
/// and again after bumping the chain and syncing
pub async fn assert_client_matches_proposal<NoteId>(
    client: &LightClient,
    proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteId>,
    txid: &TxId,
) {
    let read_lock = client
        .wallet
        .transaction_context
        .transaction_metadata_set
        .read()
        .await;
    let transaction_record = read_lock
        .transaction_records_by_id
        .get(txid)
        .expect("sender must recognize txid");

    let status = transaction_record.status;

    //todo
}

//! contains functions that compare structs to see if they match

use zcash_client_backend::{
    proposal::{Proposal, Step},
    zip321::Payment,
};
use zcash_primitives::transaction::TxId;

use zingolib::lightclient::LightClient;

/// call this after calling complete_and_broadcast_proposalx
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

    //multi-step proposals are not yet supported
    assert_eq!(proposal.steps().len(), 1);
    let step: &Step<NoteId> = proposal.steps().first();
    for (_, payment) in step.transaction_request().payments().into_iter() {
        // if wallet_includes_address(payment.recipient_address) {}
    }

    for transparent_input in step.transparent_inputs() {}
    if let Some(shielded_inputs) = step.shielded_inputs() {
        for shielded_input in shielded_inputs.notes() {}
    }

    step.balance();
}

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

    let status = transaction_record.status;

    //multi-step proposals are not yet supported
    assert_eq!(proposal.steps().len(), 1);
    let step: &Step<NoteId> = proposal.steps().first();
    for (_, payment) in step.transaction_request().payments().into_iter() {
        // assert_client_matches_payment(client, payment, status);
    }

    step.transparent_inputs();
    step.shielded_inputs();
    step.balance();
}

/// checks that a client has recorded a payment, given a certain status
pub async fn assert_client_matches_payment<NoteId>(
    client: &LightClient,
    payment: Payment,
    // status: ConfirmationStatus,
) {
    // if client.includes_address(payment.address()
}

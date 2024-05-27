//! contains functions that compare structs to see if they match

use nonempty::NonEmpty;

use zcash_client_backend::proposal::Proposal;
use zcash_primitives::transaction::TxId;
use zingolib::lightclient::LightClient;

/// assert send outputs match client
/// currently only checks if the fee matches
/// this currently fails for any broadcast but not confirmed transaction: it seems like get_transaction_fee does not recognize pending spends
pub async fn assert_send_outputs_match_client<NoteId>(
    client: &LightClient,
    proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteId>,
    txids: &NonEmpty<TxId>,
) {
    let records = &client
        .wallet
        .transaction_context
        .transaction_metadata_set
        .read()
        .await
        .transaction_records_by_id;

    assert_eq!(proposal.steps().len(), txids.len());
    for (i, step) in proposal.steps().iter().enumerate() {
        let record = records.get(&txids[i]).expect("sender must recognize txid");
        // does this record match this step?
        // may fail in uncertain ways if used on a transaction we dont have an OutgoingViewingKey for
        assert_eq!(
            records.calculate_transaction_fee(record).unwrap(),
            step.balance().fee_required().into_u64()
        );
    }
}

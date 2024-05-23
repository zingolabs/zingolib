//! contains functions that compare structs to see if they match

use nonempty::NonEmpty;

use zcash_client_backend::proposal::Proposal;
use zcash_primitives::transaction::TxId;
use zingolib::{lightclient::LightClient, wallet::notes::query::OutputQuery};

/// currently only checks if the fee matches
/// this currently fails for any broadcast but not confirmed transaction: it seems like get_transaction_fee does not recognize pending spends
pub async fn assert_send_outputs_match_sender<NoteId>(
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

/// currently only checks if the fee matches
/// this currently fails for any broadcast but not confirmed transaction: it seems like get_transaction_fee does not recognize pending spends
pub async fn assert_send_outputs_match_receiver<NoteId>(
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

        let mut sum_received = 0;
        for (_, payment) in step.transaction_request().payments() {
            sum_received = payment.amount.into_u64();
        }

        assert_eq!(sum_received, record.query_sum_value(OutputQuery::any()));
    }
}

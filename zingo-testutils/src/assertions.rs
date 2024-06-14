//! contains functions that compare structs to see if they match

use nonempty::NonEmpty;

use zcash_client_backend::proposal::Proposal;
use zcash_primitives::transaction::TxId;
use zingolib::{lightclient::LightClient, wallet::notes::query::OutputQuery};

/// currently only checks if the fee matches
/// this currently fails for any broadcast but not confirmed transaction: it seems like get_transaction_fee does not recognize pending spends
/// returns the total fee for the transfer
pub async fn assert_sender_fee<NoteId>(
    client: &LightClient,
    proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteId>,
    txids: &NonEmpty<TxId>,
) -> u64 {
    let records = &client
        .wallet
        .transaction_context
        .transaction_metadata_set
        .read()
        .await
        .transaction_records_by_id;

    assert_eq!(proposal.steps().len(), txids.len());
    let mut total_fee = 0;
    for (i, step) in proposal.steps().iter().enumerate() {
        let record = records.get(&txids[i]).expect("sender must recognize txid");
        // does this record match this step?
        // may fail in uncertain ways if used on a transaction we dont have an OutgoingViewingKey for
        let recorded_fee = records.calculate_transaction_fee(record).unwrap();
        assert_eq!(recorded_fee, step.balance().fee_required().into_u64());

        total_fee += recorded_fee;
    }
    total_fee
}

/// currently only checks if the received total matches
pub async fn assert_receiver_fee<NoteId>(
    client: &LightClient,
    proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteId>,
    txids: &NonEmpty<TxId>,
) -> u64 {
    let records = &client
        .wallet
        .transaction_context
        .transaction_metadata_set
        .read()
        .await
        .transaction_records_by_id;

    assert_eq!(proposal.steps().len(), txids.len());
    let mut total_output = 0;
    for (i, step) in proposal.steps().iter().enumerate() {
        let record = records.get(&txids[i]).expect("sender must recognize txid");

        let recorded_output = record.query_sum_value(OutputQuery::any());
        assert_eq!(
            recorded_output,
            step.transaction_request().total().unwrap().into_u64()
        );
        total_output += recorded_output;
    }
    total_output
}

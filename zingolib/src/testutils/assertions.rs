//! contains functions that compare structs to see if they match

use nonempty::NonEmpty;

use zcash_client_backend::proposal::Proposal;
use zcash_primitives::transaction::TxId;

use crate::{lightclient::LightClient, wallet::notes::query::OutputQuery};
use zingo_status::confirmation_status::ConfirmationStatus;

/// currently checks:
/// 1. len of txids == num steps
/// 2. the txid is stored in the records_by_ids database
/// 3. if the fee from the calculate_transaction_fee matches the sum of the per-step fees
/// this currently fails for any broadcast but not confirmed transaction: it seems like get_transaction_fee does not recognize pending spends
/// returns the total fee for the transfer
pub async fn assert_record_fee_and_status<NoteId>(
    client: &LightClient,
    proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteId>,
    txids: &NonEmpty<TxId>,
    expected_status: ConfirmationStatus,
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
        let record = records.get(&txids[i]).unwrap_or_else(|| {
            panic!(
                "sender must recognize txid.\nExpected {}\nRecognised: {:?}",
                txids[i],
                records.0.values().collect::<Vec<_>>()
            )
        });
        // does this record match this step?
        // we can check that it has the expected status
        assert_eq!(record.status, expected_status);
        // may fail in uncertain ways if used on a transaction we dont have an OutgoingViewingKey for
        let recorded_fee = records.calculate_transaction_fee(record).unwrap();
        assert_eq!(recorded_fee, step.balance().fee_required().into_u64());

        total_fee += recorded_fee;
    }
    total_fee
}

/// currently only checks if the received total matches
pub async fn assert_recipient_total_lte_to_proposal_total<NoteId>(
    recipient: &LightClient,
    proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteId>,
    txids: &NonEmpty<TxId>,
) -> u64 {
    let records = &recipient
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
        assert!(recorded_output <= step.transaction_request().total().unwrap().into_u64());
        total_output += recorded_output;
    }
    total_output
}

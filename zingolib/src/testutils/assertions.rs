//! contains functions that compare structs to see if they match

use nonempty::NonEmpty;

use zcash_client_backend::proposal::Proposal;
use zcash_primitives::transaction::TxId;

use crate::{lightclient::LightClient, wallet::notes::query::OutputQuery};

fn compare_fee_result(
    recorded_fee_result: &Result<u64, crate::wallet::error::FeeError>,
    proposed_fee: u64,
) -> Result<u64, ()> {
    if let Ok(recorded_fee) = recorded_fee_result {
        if *recorded_fee == proposed_fee {
            return Ok(*recorded_fee);
        }
    }
    Err(())
}

#[allow(missing_docs)] // error types document themselves
#[derive(Debug, thiserror::Error)]
pub enum ProposalToTransactionRecordComparisonError {
    #[error("TxId missing from broadcast.")]
    MissingFromBroadcast,
    #[error("Could not look up TransactionRecord.")]
    MissingRecord,
    #[error("Mismatch: Recorded fee: {0:?} ; Expected fee: {1:?}")]
    Mismatch(Result<u64, crate::wallet::error::FeeError>, u64),
}

/// currently checks:
/// 1. len of txids == num steps
/// 2. the txid is stored in the records_by_ids database
/// 3. if the fee from the calculate_transaction_fee matches the sum of the per-step fees
///
/// if any of these checks fail, rather than panic immediately, this function will include an error enum in its output. make sure to expect this.
pub async fn lookup_fees_with_proposal_check<NoteId>(
    client: &LightClient,
    proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteId>,
    txids: &NonEmpty<TxId>,
) -> Vec<Result<u64, ProposalToTransactionRecordComparisonError>> {
    let records = &client
        .wallet
        .transaction_context
        .transaction_metadata_set
        .read()
        .await
        .transaction_records_by_id;

    let mut step_results = vec![];
    for (step_number, step) in proposal.steps().iter().enumerate() {
        step_results.push({
            if let Some(txid) = txids.get(step_number) {
                if let Some(record) = records.get(txid) {
                    let recorded_fee_result = records.calculate_transaction_fee(record);
                    let proposed_fee = step.balance().fee_required().into_u64();
                    compare_fee_result(&recorded_fee_result, proposed_fee).map_err(|_| {
                        ProposalToTransactionRecordComparisonError::Mismatch(
                            recorded_fee_result,
                            proposed_fee,
                        )
                    })
                } else {
                    Err(ProposalToTransactionRecordComparisonError::MissingRecord)
                }
            } else {
                Err(ProposalToTransactionRecordComparisonError::MissingFromBroadcast)
            }
        });
    }
    step_results
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

//! contains functions that compare structs to see if they match

use nonempty::NonEmpty;

use zcash_client_backend::proposal::{Proposal, Step};
use zcash_primitives::transaction::TxId;

use crate::{
    lightclient::LightClient,
    wallet::{data::TransactionRecord, transaction_records_by_id::TransactionRecordsById},
};

#[allow(missing_docs)] // error types document themselves
#[derive(Debug, thiserror::Error)]
pub enum ProposalToTransactionRecordComparisonError {
    #[error("{0:?}")]
    LookupError(#[from] LookupRecordsPairStepsError),
    #[error("Mismatch: Recorded fee: {0:?} ; Expected fee: {1:?}")]
    Mismatch(Result<u64, crate::wallet::error::FeeError>, u64),
}

/// compares a proposal with a fulfilled record and returns the agreed fee
pub fn compare_fee<NoteRef>(
    records: &TransactionRecordsById,
    record: &TransactionRecord,
    step: &Step<NoteRef>,
) -> Result<u64, ProposalToTransactionRecordComparisonError> {
    let recorded_fee_result = records.calculate_transaction_fee(record);
    let proposed_fee = step.balance().fee_required().into_u64();
    if let Ok(recorded_fee) = recorded_fee_result {
        if recorded_fee == proposed_fee {
            return Ok(recorded_fee);
        }
    }
    Err(ProposalToTransactionRecordComparisonError::Mismatch(
        recorded_fee_result,
        proposed_fee,
    ))
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
    for_each_proposed_record(client, proposal, txids, |records, record, step| {
        compare_fee(records, record, step)
    })
    .await
    .into_iter()
    .map(|stepwise_result| {
        stepwise_result
            .map_err(ProposalToTransactionRecordComparisonError::LookupError)
            .and_then(|fee_comparison_result| fee_comparison_result)
    })
    .collect()
}

#[allow(missing_docs)] // error types document themselves
#[derive(Debug, thiserror::Error)]
pub enum LookupRecordsPairStepsError {
    #[error("TxId missing from broadcast.")]
    MissingFromBroadcast,
    #[error("Could not look up TransactionRecord with txid {0:?}.")]
    MissingRecord(TxId),
}

/// checks the client for record of each of the expected transactions, and does anything to them.
pub async fn for_each_proposed_record<NoteId, Res>(
    client: &LightClient,
    proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, NoteId>,
    txids: &NonEmpty<TxId>,
    f: fn(&TransactionRecordsById, &TransactionRecord, &Step<NoteId>) -> Res,
) -> Vec<Result<Res, LookupRecordsPairStepsError>> {
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
                    Ok(f(records, record, step))
                } else {
                    Err(LookupRecordsPairStepsError::MissingRecord(*txid))
                }
            } else {
                Err(LookupRecordsPairStepsError::MissingFromBroadcast)
            }
        });
    }
    step_results
}

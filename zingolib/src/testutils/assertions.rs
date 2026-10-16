//! contains functions that compare structs to see if they match

use nonempty::NonEmpty;

use pepper_sync::wallet::WalletTransaction;
use zcash_client_backend::proposal::{Proposal, Step};
use zcash_primitives::transaction::TxId;
use zcash_protocol::value::Zatoshis;

use crate::{lightclient::LightClient, wallet::LightWallet};

#[allow(missing_docs)] // error types document themselves
#[derive(Debug, thiserror::Error)]
pub enum ProposalToTransactionRecordComparisonError {
    #[error("{0:?}")]
    LookupError(#[from] LookupRecordsPairStepsError),
    #[error("Mismatch: Recorded fee: {0:?} ; Expected fee: {1:?}")]
    Mismatch(Result<Zatoshis, crate::wallet::error::FeeError>, Zatoshis),
}

/// compares a proposal with a fulfilled record and returns the agreed fee
pub fn compare_fee<NoteRef>(
    wallet: &LightWallet,
    transaction: &WalletTransaction,
    step: &Step<NoteRef>,
) -> Result<Zatoshis, ProposalToTransactionRecordComparisonError> {
    let recorded_fee_result = wallet.calculate_transaction_fee(transaction);
    let proposed_fee = step.balance().fee_required();
    if let Ok(recorded_fee) = recorded_fee_result
        && recorded_fee == proposed_fee
    {
        return Ok(recorded_fee);
    }
    Err(ProposalToTransactionRecordComparisonError::Mismatch(
        recorded_fee_result,
        proposed_fee,
    ))
}

/// currently checks:
/// 1. len of txids == num steps
/// 2. the txid is stored in the `records_by_ids` database
/// 3. if the fee from the `calculate_transaction_fee` matches the sum of the per-step fees
///
/// if any of these checks fail, rather than panic immediately, this function will include an error enum in its output. make sure to expect this.
pub async fn lookup_fees_with_proposal_check<N>(
    client: &LightClient,
    proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, N>,
    txids: &NonEmpty<TxId>,
) -> Vec<Result<Zatoshis, ProposalToTransactionRecordComparisonError>> {
    for_each_proposed_transaction(client, proposal, txids, |records, record, step| {
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
    #[error("TxId missing from transmission.")]
    MissingFromTransmission,
    #[error("Could not look up TransactionRecord with txid {0:?}.")]
    MissingRecord(TxId),
}

/// checks the client for record of each of the expected transactions, and does anything to them.
pub async fn for_each_proposed_transaction<N, Res>(
    client: &LightClient,
    proposal: &Proposal<zcash_primitives::transaction::fees::zip317::FeeRule, N>,
    txids: &NonEmpty<TxId>,
    f: fn(&LightWallet, &WalletTransaction, &Step<N>) -> Res,
) -> Vec<Result<Res, LookupRecordsPairStepsError>> {
    let wallet = client.wallet().read().await;

    let mut step_results = vec![];
    for (step_number, step) in proposal.steps().iter().enumerate() {
        step_results.push({
            if let Some(txid) = txids.get(step_number) {
                if let Some(transaction) = wallet.wallet_transactions.get(txid) {
                    Ok(f(&wallet, transaction, step))
                } else {
                    Err(LookupRecordsPairStepsError::MissingRecord(*txid))
                }
            } else {
                Err(LookupRecordsPairStepsError::MissingFromTransmission)
            }
        });
    }
    step_results
}

/// The null-data opcode that opens an OP_RETURN script.
pub const OP_RETURN: u8 = 0x6a;

/// - Panics unless `bundle` is the one-input, two-output shape of an OP_RETURN send whose zero-value
///   output's script opens with OP_RETURN and carries `payload` verbatim.
pub fn assert_null_data_send(
    bundle: &zcash_transparent::bundle::Bundle<zcash_transparent::bundle::Authorized>,
    payload: &[u8],
) {
    assert_eq!(bundle.vin.len(), 1, "spends exactly one output");
    assert_eq!(
        bundle.vout.len(),
        2,
        "recipient + OP_RETURN, no change output"
    );
    let op_return_out = bundle
        .vout
        .iter()
        .find(|out| out.value() == zcash_protocol::value::Zatoshis::ZERO)
        .expect("a zero-value OP_RETURN output");
    let script = op_return_out.script_pubkey().0.0.as_slice();
    assert_eq!(
        script.first(),
        Some(&OP_RETURN),
        "null-data script starts with OP_RETURN"
    );
    assert!(
        script
            .windows(payload.len())
            .any(|window| window == payload),
        "the OP_RETURN carries the payload verbatim"
    );
    assert!(
        bundle
            .vout
            .iter()
            .any(|out| out.value() != zcash_protocol::value::Zatoshis::ZERO),
        "the recipient output is the non-null-data output"
    );
}

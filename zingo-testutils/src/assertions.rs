//! contains functions that compare structs to see if they match

use nonempty::NonEmpty;

use zcash_client_backend::proposal::Proposal;
use zcash_primitives::transaction::TxId;
use zingolib::{lightclient::LightClient, wallet::notes::query::OutputQuery};

/// currently only checks if the fee matches
/// this currently fails for any broadcast but not confirmed transaction: it seems like get_transaction_fee does not recognize pending spends
/// returns the total fee for the transfer
pub async fn assert_send_outputs_match_sender<NoteId>(
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
pub async fn assert_send_outputs_match_recipient<NoteId>(
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

        assert_eq!(
            record.query_sum_value(OutputQuery::any()),
            step.transaction_request().total().unwrap().into_u64()
        );
    }
}

/// assert that proposing shield causes a specific error
pub async fn assert_cant_shield(client: &zingolib::lightclient::LightClient) {
    assert!(match client.propose_shield().await.unwrap_err() {
            zingolib::lightclient::propose::ProposeShieldError::Component(zcash_client_backend::data_api::error::Error::NoteSelection(zcash_client_backend::data_api::wallet::input_selection::GreedyInputSelectorError::Change(zcash_client_backend::fees::ChangeError::InsufficientFunds { available, required }))) => {
                println!("available {}, required more than {}", available.into_u64(), required.into_u64());
                true
            },
            zingolib::lightclient::propose::ProposeShieldError::BelowThreshold(amount) => {
                println!("possible note {}, threshold {}", amount, zingolib::lightclient::propose::SHIELDING_CUTOFF);
                true},
            e => {
                dbg!(e);
                false
            }
        });
}

//! contains functions that compare structs to see if they match

use zcash_client_backend::proposal::Proposal;
use zcash_client_backend::proposal::Step;

use zcash_client_backend::ShieldedProtocol::Orchard;
use zcash_client_backend::ShieldedProtocol::Sapling;
use zcash_primitives::transaction::TxId;

use zingolib::lightclient::LightClient;
use zingolib::wallet::notes::query::OutputPoolQuery;
use zingolib::wallet::notes::query::OutputQuery;
use zingolib::wallet::notes::query::OutputSpendStatusQuery;

/// call this after calling complete_and_broadcast_proposalx
/// and again after bumping the chain and syncing
pub async fn assert_sender_understands_proposal<NoteId>(
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

    // assert outgoing payments are recorded. tbi
    // for (_, payment) in step.transaction_request().payments().into_iter() {
    //     // if wallet_includes_address(payment.recipient_address) {}
    // }

    // assert transparent inputs are marked as spent. this will be written once a test includes transparent inputs.
    // for transparent_input in step.transparent_inputs() {
    //     assert!(read_lock
    //         .transaction_records_by_id
    //         .values()
    //         .find(|transparent_record| {
    //             transaction_record
    //                 .transparent_outputs
    //                 .iter()
    //                 .find(|transparent_output| {
    //                     transparent_output.value == transparent_input.txout().value.into_u64()
    //                 })
    //                 .is_some()
    //         })
    //         .is_some());
    // }

    // assert that shielded inputs are marked as spent.
    if let Some(shielded_inputs) = step.shielded_inputs() {
        for _shielded_input in shielded_inputs.notes() {}
    }

    let balance = step.balance();

    assert_eq!(
        transaction_record.query_sum_value(OutputQuery::new(
            OutputSpendStatusQuery {
                unspent: true,
                pending_spent: false,
                spent: false,
            },
            OutputPoolQuery {
                transparent: false,
                sapling: false,
                orchard: true, // change is sent to orchard
            }
        )), // this will break on the send_to_self_edge_case
        balance
            .proposed_change()
            .iter()
            .fold(0, |sum, change_value| {
                if change_value.output_pool() == Orchard {
                    sum + change_value.value().into_u64()
                } else {
                    sum
                }
            })
    );
    assert!(!balance
        .proposed_change()
        .iter()
        .any(|change_value| change_value.output_pool() == Sapling)); // no expected sapling change
}

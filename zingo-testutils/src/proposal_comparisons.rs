use zcash_client_backend::proposal::Proposal;
use zcash_primitives::transaction::fees::zip317::FeeRule;
use zingolib::lightclient::LightClient;

/// asserts that a proposal has been ingested cleanly by a wallet
pub async fn compare_proposal_to_sender_pending<NoteId>(
    proposal: Proposal<NoteId, FeeRule>,
    sender: LightClient,
) -> bool {
    for transparent_input in proposal.steps().first().transparent_inputs() {
        // todo assert that these outputs are pending spent
    }
    for shielded_input in proposal.steps().first().shielded_inputs().unwrap().notes() {
        let shielded_input_id = shielded_input.internal_note_id();
        // todo assert that these notes are pending spent
    }
    true
}

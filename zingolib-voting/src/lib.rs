//! Consumer-ready shielded voting for zingolib's LightClient.
//!
//! This crate owns the voting lifecycle so `LightClient` can expose it with
//! thin delegating methods. The consumer's shape follows the repo's
//! progress-handle idiom: [`begin_voting`] hands back a [`VotingProgressHandle`]
//! over a typed [`VotingEvent`] channel, and the lifecycle that
//! [`cast_vote`] drives publishes into it. Logic lives here; the client
//! supplies wallet-derived inputs ([`VotingSnapshot`]) and the software
//! signer inside its seed boundary.
//!
//! Re-exports keep the SDK's input contract and stable types reachable from
//! zingolib without a second dependency edge on `zcash_voting`.

pub mod config;
pub mod error;
pub mod events;
pub mod journal;
pub mod session;
pub mod vote_server;
pub mod witness;

pub use config::{
    PRODUCTION_STATIC_VOTING_CONFIG_SOURCE, STAGING_STATIC_VOTING_CONFIG_SOURCE,
    STATIC_CONFIG_SOURCE_ENV, resolve_voting_config,
};
pub use error::{VotingFailure, VotingFailureKind, VotingFailureSummary};
pub use events::{VotingEvent, VotingProgressHandle};
pub use journal::BallotDecision;
pub use session::{
    ActiveRoundSummary, ArmedVotingSession, PastBallot, VotingResults, VotingRoundOutcome,
    VotingSessionConfig, VotingSnapshot, arm_session, cast_ballot, list_network_rounds,
    past_ballots, resolve_active_round, voting_results,
};
pub use vote_server::{
    ActiveVotingRound, ServerProposal, ServerRound, TallyRow, authenticate_round,
    endorsed_round_ids, fetch_rounds, tally_results,
};
pub use zcash_voting::DirectRoute;
pub use zcash_voting::note_bundling::{
    BundlePolicy, minimum_voting_eligibility_and_plan_for_notes,
};
pub use zcash_voting::round::VotingDb;
pub use zcash_voting::session::Decision;
pub use zcash_voting::storage::RoundPhase;
pub use zcash_voting::types::VotingError;
pub use zcash_voting::vote_work::BallotIntent;

pub use zcash_voting::DelegationSigner;
/// The SDK surface the session and its inputs are built from, re-exported so
/// zingolib (and a future FFI layer) names one dependency.
pub use zcash_voting::backend::pasta_curves;
pub use zcash_voting::delegate::{DelegationAccountKeys, DelegationKeys, DelegationSigningRequest};
pub use zcash_voting::types::{Network, NoteInfo, VotingHotkey, WitnessData};
pub use zcash_voting::wire::{DraftVote, VotingRoundParams};

#[cfg(test)]
mod tests {
    use super::*;

    /// The journal and the session agree on the decision type, so the
    /// consumer journals through one name.
    #[test]
    fn decision_reexport_round_trips() {
        let skipped = Decision::Skipped;
        let chosen = Decision::Choice(1);
        assert!(!matches!(skipped, Decision::Choice(_)));
        assert!(matches!(chosen, Decision::Choice(1)));
    }
}

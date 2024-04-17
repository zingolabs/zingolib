//! The types of transaction Proposal that Zingo! uses.
use std::convert::Infallible;

use zcash_client_backend::proposal::Proposal;
use zcash_primitives::transaction::fees::zip317::FeeRule;

use crate::wallet::notes::NoteRecordIdentifier;

/// A proposed send to addresses.
pub type TransferProposal = Proposal<FeeRule, NoteRecordIdentifier>;
/// A proposed shielding.
/// The zcash_client_backend Proposal type exposes a "NoteRef" generic
/// parameter to track Shielded inputs to the proposal these are
/// disallowed ing Zingo ShielddedProposals
pub type ShieldProposal = Proposal<FeeRule, Infallible>;

/// The LightClient holds one proposal at a time while the user decides whether to accept the fee.
#[derive(Clone)]
pub enum ZingoProposal {
    /// Destination somewhere else. This is an implementation detail of the code as is, not
    /// an invariant that is guaranteed for any number of versions.
    Transfer(TransferProposal),
    /// Destination this wallet. This is an implementation detail of the code as is, not
    /// an invariant that is guaranteed for any number of versions.
    Shield(ShieldProposal),
}

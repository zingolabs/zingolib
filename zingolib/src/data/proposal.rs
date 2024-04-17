//! The types of transaction Proposal that Zingo! uses.
use std::convert::Infallible;

use zcash_client_backend::proposal::Proposal;
use zcash_primitives::transaction::fees::zip317::FeeRule;

use crate::wallet::notes::NoteRecordIdentifier;

/// A proposed send to addresses.
/// Identifies the notes to spend by txid, pool, and output_index.
pub(crate) type TransferProposal = Proposal<FeeRule, NoteRecordIdentifier>;
/// A proposed shielding.
/// The zcash_client_backend Proposal type exposes a "NoteRef" generic
/// parameter to track Shielded inputs to the proposal these are
/// disallowed in Zingo ShieldedProposals
pub(crate) type ShieldProposal = Proposal<FeeRule, Infallible>;

/// The LightClient holds one proposal at a time while the user decides whether to accept the fee.
#[derive(Clone)]
pub(crate) enum ZingoProposal {
    /// Destination somewhere else.
    /// Can propose any valid recipient.
    Transfer(TransferProposal),
    /// For now this is constrained by lrz zcash_client_backend transaction construction
    /// to send to the proposing capability's receiver for its fanciest shielded pool
    Shield(ShieldProposal),
}

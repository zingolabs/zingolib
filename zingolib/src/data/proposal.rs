//! The types of transaction Proposal that Zingo! uses.
use std::convert::Infallible;

use zcash_client_backend::proposal::Proposal;
use zcash_primitives::transaction::fees::zip317::FeeRule;

use crate::wallet::notes::NoteRecordIdentifier;

/// A proposed send to addresses.
pub type TransferProposal = Proposal<FeeRule, NoteRecordIdentifier>;
/// A proposed shielding.
/// The zcash_client_backend Proposal type exposes a "NoteRef" generic
/// parameter to track notes that are generated in proposal steps,
/// but Shielding has no $INSERT CORRECT TERMinilogy HERE steps that produce
/// outputs which are consume in the same proposal, rather the outputs
/// are unspent shielded notes.
pub type ShieldProposal = Proposal<FeeRule, Infallible>;

/// The LightClient holds one proposal at a time while the user decides whether to accept the fee.
#[derive(Clone)]
pub enum ZingoProposal {
    /// Destination somewhere else.
    Transfer(TransferProposal),
    /// Destination this wallet.
    Shield(ShieldProposal),
}

//! The types of transaction Proposal that Zingo! uses.
use std::convert::Infallible;

use zcash_client_backend::proposal::Proposal;
use zcash_primitives::transaction::fees::zip317::FeeRule;

use crate::wallet::notes::NoteRecordIdentifier;

/// A proposed send to addresses.
pub(crate) type TransferProposal = Proposal<FeeRule, NoteRecordIdentifier>;
/// A proposed shielding.
pub(crate) type ShieldProposal = Proposal<FeeRule, Infallible>;

/// The LightClient holds one proposal at a time while the user decides whether to accept the fee.
#[derive(Clone)]
pub(crate) enum ZingoProposal {
    /// Destination somewhere else.
    Transfer(TransferProposal),
    /// Destination this wallet.
    Shield(ShieldProposal),
}

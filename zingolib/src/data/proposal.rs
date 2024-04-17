use std::convert::Infallible;

use zcash_client_backend::proposal::Proposal;
use zcash_primitives::transaction::fees::zip317::FeeRule;

use crate::wallet::notes::NoteRecordIdentifier;

pub type TransferProposal = Proposal<FeeRule, NoteRecordIdentifier>;
pub type ShieldProposal = Proposal<FeeRule, Infallible>;
#[derive(Clone)]
pub enum ZingoProposal {
    Transfer(TransferProposal),
    Shield(ShieldProposal),
}

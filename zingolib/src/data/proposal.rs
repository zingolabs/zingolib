//! The types of transaction Proposal that Zingo! uses.

use std::convert::Infallible;

use zcash_client_backend::proposal::Proposal;
use zcash_primitives::transaction::{
    components::amount::{BalanceError, NonNegativeAmount},
    fees::zip317::FeeRule,
};

/// A proposed send to addresses.
/// Identifies the notes to spend by txid, pool, and output_index.
pub type TransferProposal = Proposal<FeeRule, zcash_client_backend::wallet::NoteId>;
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
    #[allow(dead_code)] // TOdo use it
    Transfer(TransferProposal),
    /// For now this is constrained by lrz zcash_client_backend transaction construction
    /// to send to the proposing capability's receiver for its fanciest shielded pool
    #[allow(dead_code)] // TOdo construct it
    Shield(ShieldProposal),
}

/// total sum of all transaction request payment amounts in a proposal
/// TODO: test for multi-step, zip320 currently unsupported.
pub fn total_payment_amount(
    proposal: &TransferProposal,
) -> Result<NonNegativeAmount, BalanceError> {
    proposal
        .steps()
        .iter()
        .map(|step| step.transaction_request())
        .fold(Ok(NonNegativeAmount::ZERO), |acc, tr| {
            (acc? + tr.total()?).ok_or(BalanceError::Overflow)
        })
}

/// total sum of all fees in a proposal
/// TODO: test for multi-step, zip320 currently unsupported.
pub fn total_fee(proposal: &TransferProposal) -> Result<NonNegativeAmount, BalanceError> {
    proposal
        .steps()
        .iter()
        .map(|step| step.balance().fee_required())
        .fold(Ok(NonNegativeAmount::ZERO), |acc, fee| {
            (acc? + fee).ok_or(BalanceError::Overflow)
        })
}

#[cfg(test)]
mod tests {
    use zcash_primitives::transaction::components::amount::NonNegativeAmount;

    use crate::mocks;

    #[test]
    fn total_payment_amount() {
        let proposal = mocks::proposal::ProposalBuilder::default().build();
        assert_eq!(
            super::total_payment_amount(&proposal).unwrap(),
            NonNegativeAmount::from_u64(100_000).unwrap()
        );
    }
    #[test]
    fn total_fee() {
        let proposal = mocks::proposal::ProposalBuilder::default().build();
        assert_eq!(
            super::total_fee(&proposal).unwrap(),
            NonNegativeAmount::from_u64(20_000).unwrap()
        );
    }
}

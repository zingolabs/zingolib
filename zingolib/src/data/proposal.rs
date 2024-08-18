//! The types of transaction Proposal that Zingo! uses.

use std::convert::Infallible;

use zcash_client_backend::proposal::Proposal;
use zcash_primitives::transaction::{
    self,
    components::amount::{BalanceError, NonNegativeAmount},
};

/// A proposed send to addresses.
/// Identifies the notes to spend by txid, pool, and output_index.
/// This type alias, specifies the ZIP317 "Proportional Transfer Fee Mechanism"structure
/// <https://zips.z.cash/zip-0317>
/// as the fee structure for a transaction series.  This innovation was created in response
/// "Binance Constraint" that t-addresses that only receive from t-addresses be supported.
/// <https://zips.z.cash/zip-0320>
pub type ProportionalFeeProposal =
    Proposal<transaction::fees::zip317::FeeRule, zcash_client_backend::wallet::NoteId>;
/// A proposed shielding.
/// The zcash_client_backend Proposal type exposes a "NoteRef" generic
/// parameter to track Shielded inputs to the proposal these are
/// disallowed in Zingo ShieldedProposals
pub(crate) type ProportionalFeeShieldProposal =
    Proposal<transaction::fees::zip317::FeeRule, Infallible>;

/// The LightClient holds one proposal at a time while the user decides whether to accept the fee.
#[derive(Clone)]
pub(crate) enum ZingoProposal {
    /// Destination somewhere else.
    /// Can propose any valid recipient.
    Transfer(ProportionalFeeProposal),
    /// For now this is constrained by lrz zcash_client_backend transaction construction
    /// to send to the proposing capability's receiver for its fanciest shielded pool
    Shield(ProportionalFeeShieldProposal),
}

/// total sum of all transaction request payment amounts in a proposal
/// TODO: test for multi-step, zip320 currently unsupported.
pub fn total_payment_amount(
    proposal: &ProportionalFeeProposal,
) -> Result<NonNegativeAmount, BalanceError> {
    proposal
        .steps()
        .iter()
        .map(|step| step.transaction_request())
        .try_fold(NonNegativeAmount::ZERO, |acc, request| {
            (acc + request.total()?).ok_or(BalanceError::Overflow)
        })
}

/// total sum of all fees in a proposal
/// TODO: test for multi-step, zip320 currently unsupported.
pub fn total_fee(proposal: &ProportionalFeeProposal) -> Result<NonNegativeAmount, BalanceError> {
    proposal
        .steps()
        .iter()
        .map(|step| step.balance().fee_required())
        .try_fold(NonNegativeAmount::ZERO, |acc, fee| {
            (acc + fee).ok_or(BalanceError::Overflow)
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

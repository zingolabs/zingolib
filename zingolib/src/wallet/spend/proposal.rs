//! The owned proposal types of the in-tree spend pipeline (ADR 0010).
//!
//! A proposal is a pure value. The plan layer produces one from a
//! read-only wallet view, the wallet stores it while the caller decides
//! whether to accept the fee (ADR 0006), and the build layer consumes it
//! without further wallet reads. The two legal multi-transaction shapes
//! zingolib produces are the enum's variants, so the states the old code
//! rejected at runtime (`NonTexMultiStep`) are unrepresentable here.

use std::collections::BTreeMap;

use zcash_protocol::PoolType;
use zcash_protocol::consensus::BlockHeight;
use zcash_protocol::memo::MemoBytes;
use zcash_protocol::value::{BalanceError, Zatoshis};
use zip321::TransactionRequest;

use pepper_sync::wallet::OutputId;

use super::op_return::OpReturnData;
use crate::wallet::output::OutputRef;

/// A shielded note selected as an input to a [`Step`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShieldedInput {
    note: OutputRef,
    value: Zatoshis,
}

impl ShieldedInput {
    /// Constructs a shielded input from its parts.
    #[must_use]
    pub fn from_parts(note: OutputRef, value: Zatoshis) -> Self {
        ShieldedInput { note, value }
    }

    /// The selected note, identified by txid, pool, and output index.
    #[must_use]
    pub fn note(&self) -> &OutputRef {
        &self.note
    }

    /// The note's value.
    #[must_use]
    pub fn value(&self) -> Zatoshis {
        self.value
    }
}

/// A transparent coin selected as an input to a [`Step`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransparentInput {
    coin: OutputId,
    value: Zatoshis,
}

impl TransparentInput {
    /// Constructs a transparent input from its parts.
    #[must_use]
    pub fn from_parts(coin: OutputId, value: Zatoshis) -> Self {
        TransparentInput { coin, value }
    }

    /// The selected coin, identified by txid and output index.
    #[must_use]
    pub fn coin(&self) -> OutputId {
        self.coin
    }

    /// The coin's value.
    #[must_use]
    pub fn value(&self) -> Zatoshis {
        self.value
    }
}

/// A change output a [`Step`] will create.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeValue {
    pool: PoolType,
    value: Zatoshis,
    memo: Option<MemoBytes>,
}

impl ChangeValue {
    /// Constructs a change output from its parts.
    #[must_use]
    pub fn from_parts(pool: PoolType, value: Zatoshis, memo: Option<MemoBytes>) -> Self {
        ChangeValue { pool, value, memo }
    }

    /// The pool the change returns to.
    #[must_use]
    pub fn pool(&self) -> PoolType {
        self.pool
    }

    /// The change value.
    #[must_use]
    pub fn value(&self) -> Zatoshis {
        self.value
    }

    /// The memo the change output carries, if any.
    #[must_use]
    pub fn memo(&self) -> Option<&MemoBytes> {
        self.memo.as_ref()
    }
}

/// One transaction of a [`Proposal`]: its payments, selected inputs,
/// change, fee, and optional OP_RETURN Data.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    transaction_request: TransactionRequest,
    payment_pools: BTreeMap<usize, PoolType>,
    shielded_inputs: Vec<ShieldedInput>,
    transparent_inputs: Vec<TransparentInput>,
    change: Vec<ChangeValue>,
    fee: Zatoshis,
    op_return_data: Option<OpReturnData>,
}

impl Step {
    /// Constructs a step from its parts.
    ///
    /// Shape rules — which steps of which proposal kinds may carry
    /// OP_RETURN Data or spend which input kinds — are enforced by the
    /// [`Proposal`] constructors, the sole place a step joins a proposal.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn from_parts(
        transaction_request: TransactionRequest,
        payment_pools: BTreeMap<usize, PoolType>,
        shielded_inputs: Vec<ShieldedInput>,
        transparent_inputs: Vec<TransparentInput>,
        change: Vec<ChangeValue>,
        fee: Zatoshis,
        op_return_data: Option<OpReturnData>,
    ) -> Self {
        Step {
            transaction_request,
            payment_pools,
            shielded_inputs,
            transparent_inputs,
            change,
            fee,
            op_return_data,
        }
    }

    /// The payments this step fulfills.
    #[must_use]
    pub fn transaction_request(&self) -> &TransactionRequest {
        &self.transaction_request
    }

    /// The pool each payment is sent to, keyed by payment index.
    #[must_use]
    pub fn payment_pools(&self) -> &BTreeMap<usize, PoolType> {
        &self.payment_pools
    }

    /// The shielded notes this step spends.
    #[must_use]
    pub fn shielded_inputs(&self) -> &[ShieldedInput] {
        &self.shielded_inputs
    }

    /// The transparent coins this step spends.
    #[must_use]
    pub fn transparent_inputs(&self) -> &[TransparentInput] {
        &self.transparent_inputs
    }

    /// The change outputs this step creates.
    #[must_use]
    pub fn change(&self) -> &[ChangeValue] {
        &self.change
    }

    /// The ZIP 317 fee this step pays.
    #[must_use]
    pub fn fee(&self) -> Zatoshis {
        self.fee
    }

    /// The OP_RETURN Data this step carries, if any.
    #[must_use]
    pub fn op_return_data(&self) -> Option<&OpReturnData> {
        self.op_return_data.as_ref()
    }

    /// The total requested payment value of this step. Payments with no
    /// stated amount contribute zero.
    ///
    /// # Errors
    ///
    /// Returns [`BalanceError`] if the payment amounts overflow.
    pub fn payment_total(&self) -> Result<Zatoshis, BalanceError> {
        Ok(self.transaction_request.total()?.unwrap_or(Zatoshis::ZERO))
    }
}

/// A shape a [`Proposal`] constructor refuses to represent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProposalShapeError {
    /// A Shield never carries OP_RETURN Data: it pays no external party.
    #[error("a Shield never carries OP_RETURN Data")]
    OpReturnOnShield,
    /// OP_RETURN Data rides only the final transaction of a send.
    #[error(
        "OP_RETURN Data rides only the final transaction of a send; \
         the shielding step of a TEX transfer cannot carry it"
    )]
    OpReturnBeforeFinalStep,
    /// A Shield spends the wallet's transparent value only.
    #[error("a Shield spends only transparent value; it cannot take shielded inputs")]
    ShieldedInputsOnShield,
}

/// A single-transaction transfer to the requested recipients.
#[derive(Debug, Clone, PartialEq)]
pub struct TransferProposal {
    account_id: zip32::AccountId,
    target_height: BlockHeight,
    step: Step,
}

impl TransferProposal {
    /// Constructs a transfer proposal. Every step shape is legal here,
    /// including OP_RETURN Data on the (single, hence final) step.
    #[must_use]
    pub fn new(account_id: zip32::AccountId, target_height: BlockHeight, step: Step) -> Self {
        TransferProposal {
            account_id,
            target_height,
            step,
        }
    }

    /// The proposal's single step.
    #[must_use]
    pub fn step(&self) -> &Step {
        &self.step
    }
}

/// The ZIP 320 two-transaction flow paying a TEX address: a shielding
/// step into an ephemeral Refund Address, then an exposure step spending
/// it to the recipient.
#[derive(Debug, Clone, PartialEq)]
pub struct TexTransferProposal {
    account_id: zip32::AccountId,
    target_height: BlockHeight,
    shielding: Step,
    exposure: Step,
}

impl TexTransferProposal {
    /// Constructs a TEX transfer proposal.
    ///
    /// # Errors
    ///
    /// Returns [`ProposalShapeError::OpReturnBeforeFinalStep`] if the
    /// shielding step carries OP_RETURN Data — only the exposure step,
    /// the flow's final transaction, may.
    pub fn new(
        account_id: zip32::AccountId,
        target_height: BlockHeight,
        shielding: Step,
        exposure: Step,
    ) -> Result<Self, ProposalShapeError> {
        if shielding.op_return_data().is_some() {
            return Err(ProposalShapeError::OpReturnBeforeFinalStep);
        }

        Ok(TexTransferProposal {
            account_id,
            target_height,
            shielding,
            exposure,
        })
    }

    /// The first step: shielded funds move to an ephemeral Refund Address.
    #[must_use]
    pub fn shielding(&self) -> &Step {
        &self.shielding
    }

    /// The final step: the Refund Address's coin pays the TEX recipient.
    #[must_use]
    pub fn exposure(&self) -> &Step {
        &self.exposure
    }
}

/// A shielding of the wallet's transparent value into the shielded pool.
#[derive(Debug, Clone, PartialEq)]
pub struct ShieldProposal {
    account_id: zip32::AccountId,
    target_height: BlockHeight,
    step: Step,
}

impl ShieldProposal {
    /// Constructs a shield proposal.
    ///
    /// # Errors
    ///
    /// Returns [`ProposalShapeError::OpReturnOnShield`] if the step
    /// carries OP_RETURN Data, or
    /// [`ProposalShapeError::ShieldedInputsOnShield`] if it spends
    /// shielded notes.
    pub fn new(
        account_id: zip32::AccountId,
        target_height: BlockHeight,
        step: Step,
    ) -> Result<Self, ProposalShapeError> {
        if step.op_return_data().is_some() {
            return Err(ProposalShapeError::OpReturnOnShield);
        }
        if !step.shielded_inputs().is_empty() {
            return Err(ProposalShapeError::ShieldedInputsOnShield);
        }

        Ok(ShieldProposal {
            account_id,
            target_height,
            step,
        })
    }

    /// The proposal's single step.
    #[must_use]
    pub fn step(&self) -> &Step {
        &self.step
    }
}

/// A spend plan the wallet has fully determined but not yet built.
///
/// The variants are the only transaction shapes zingolib produces, so a
/// value of this type is always buildable — no variant needs runtime
/// shape rejection.
#[derive(Debug, Clone, PartialEq)]
pub enum Proposal {
    /// A single-transaction transfer.
    Transfer(TransferProposal),
    /// The ZIP 320 two-transaction TEX flow.
    TexTransfer(TexTransferProposal),
    /// A shielding of transparent value.
    Shield(ShieldProposal),
}

impl Proposal {
    /// The account this proposal spends from.
    #[must_use]
    pub fn account_id(&self) -> zip32::AccountId {
        match self {
            Proposal::Transfer(p) => p.account_id,
            Proposal::TexTransfer(p) => p.account_id,
            Proposal::Shield(p) => p.account_id,
        }
    }

    /// The chain height the proposal targets. The built transactions'
    /// expiry is derived from it.
    #[must_use]
    pub fn target_height(&self) -> BlockHeight {
        match self {
            Proposal::Transfer(p) => p.target_height,
            Proposal::TexTransfer(p) => p.target_height,
            Proposal::Shield(p) => p.target_height,
        }
    }

    /// The proposal's steps in build order.
    #[must_use]
    pub fn steps(&self) -> Vec<&Step> {
        match self {
            Proposal::Transfer(p) => vec![&p.step],
            Proposal::TexTransfer(p) => vec![&p.shielding, &p.exposure],
            Proposal::Shield(p) => vec![&p.step],
        }
    }

    /// The step that pays the requested recipients — the flow's final
    /// transaction, the only one that may carry OP_RETURN Data.
    #[must_use]
    pub fn final_step(&self) -> &Step {
        match self {
            Proposal::Transfer(p) => &p.step,
            Proposal::TexTransfer(p) => &p.exposure,
            Proposal::Shield(p) => &p.step,
        }
    }

    /// The OP_RETURN Data the proposal carries, if any.
    #[must_use]
    pub fn op_return_data(&self) -> Option<&OpReturnData> {
        self.final_step().op_return_data()
    }

    /// The sum of all steps' fees.
    ///
    /// # Errors
    ///
    /// Returns [`BalanceError::Overflow`] if the fees overflow.
    pub fn total_fee(&self) -> Result<Zatoshis, BalanceError> {
        self.steps()
            .iter()
            .map(|step| step.fee())
            .try_fold(Zatoshis::ZERO, |acc, fee| {
                (acc + fee).ok_or(BalanceError::Overflow)
            })
    }

    /// The sum of all steps' requested payment values.
    ///
    /// # Errors
    ///
    /// Returns [`BalanceError`] if the payment amounts overflow.
    pub fn total_payment_amount(&self) -> Result<Zatoshis, BalanceError> {
        self.steps()
            .iter()
            .map(|step| step.payment_total())
            .try_fold(Zatoshis::ZERO, |acc, total| {
                (acc + total?).ok_or(BalanceError::Overflow)
            })
    }

    /// The same proposal aimed at a different target height — the pure
    /// retarget of ADR 0008. Policy (when to retarget, never lowering a
    /// stored target) belongs to the caller; this is the mechanism only.
    #[must_use]
    pub fn with_target_height(mut self, target_height: BlockHeight) -> Self {
        match &mut self {
            Proposal::Transfer(p) => p.target_height = target_height,
            Proposal::TexTransfer(p) => p.target_height = target_height,
            Proposal::Shield(p) => p.target_height = target_height,
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use zcash_protocol::PoolType;
    use zcash_protocol::value::Zatoshis;

    use super::{Proposal, ProposalShapeError, ShieldProposal, Step, TexTransferProposal};
    use crate::mocks::proposal::TransactionRequestBuilder;
    use crate::wallet::spend::op_return::OpReturnData;

    fn step(fee: u64, op_return_data: Option<OpReturnData>) -> Step {
        Step::from_parts(
            TransactionRequestBuilder::default().build(),
            BTreeMap::from([(0, PoolType::ORCHARD)]),
            vec![],
            vec![],
            vec![],
            Zatoshis::const_from_u64(fee),
            op_return_data,
        )
    }

    fn data() -> OpReturnData {
        OpReturnData::new(b"=:ZEC.ZEC:example".to_vec()).unwrap()
    }

    #[test]
    fn shield_refuses_op_return_data() {
        assert_eq!(
            ShieldProposal::new(
                zip32::AccountId::ZERO,
                100.into(),
                step(10_000, Some(data()))
            )
            .unwrap_err(),
            ProposalShapeError::OpReturnOnShield,
        );
    }

    #[test]
    fn tex_transfer_refuses_op_return_data_on_the_shielding_step() {
        assert_eq!(
            TexTransferProposal::new(
                zip32::AccountId::ZERO,
                100.into(),
                step(10_000, Some(data())),
                step(15_000, None),
            )
            .unwrap_err(),
            ProposalShapeError::OpReturnBeforeFinalStep,
        );
    }

    #[test]
    fn tex_transfer_carries_op_return_data_on_the_exposure_step() {
        let proposal = Proposal::TexTransfer(
            TexTransferProposal::new(
                zip32::AccountId::ZERO,
                100.into(),
                step(10_000, None),
                step(15_000, Some(data())),
            )
            .unwrap(),
        );

        assert_eq!(proposal.op_return_data(), Some(&data()));
        assert_eq!(
            proposal.final_step().fee(),
            Zatoshis::const_from_u64(15_000)
        );
    }

    #[test]
    fn totals_sum_across_steps() {
        let proposal = Proposal::TexTransfer(
            TexTransferProposal::new(
                zip32::AccountId::ZERO,
                100.into(),
                step(10_000, None),
                step(15_000, None),
            )
            .unwrap(),
        );

        assert_eq!(
            proposal.total_fee().unwrap(),
            Zatoshis::const_from_u64(25_000)
        );
        // The default mock request pays 100_000 zatoshis per step.
        assert_eq!(
            proposal.total_payment_amount().unwrap(),
            Zatoshis::const_from_u64(200_000)
        );
    }

    #[test]
    fn retarget_changes_only_the_target_height() {
        let proposal = Proposal::Shield(
            ShieldProposal::new(zip32::AccountId::ZERO, 100.into(), step(10_000, None)).unwrap(),
        );

        let retargeted = proposal.clone().with_target_height(500.into());

        assert_eq!(retargeted.target_height(), 500.into());
        assert_eq!(retargeted.steps(), proposal.steps());
        assert_eq!(retargeted.account_id(), proposal.account_id());
    }
}

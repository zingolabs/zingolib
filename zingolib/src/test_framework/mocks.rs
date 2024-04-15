//! Tools to facilitate mocks for testing

use std::collections::BTreeMap;

use nonempty::NonEmpty;
use zcash_client_backend::proposal::{Proposal, ShieldedInputs, Step, StepOutput};
use zcash_client_backend::wallet::WalletTransparentOutput;
use zcash_client_backend::PoolType;
use zcash_client_backend::{fees::TransactionBalance, zip321::TransactionRequest};
use zcash_primitives::transaction::{components::amount::NonNegativeAmount, fees::zip317::FeeRule};
use zcash_primitives::{consensus::BlockHeight, transaction::TxId};

use crate::wallet::notes::TransparentNote;

macro_rules! build_method {
    ($name:ident, $localtype:ty) => {
        pub fn $name(mut self, $name: $localtype) -> Self {
            self.$name = Some($name);
            self
        }
    };
}

pub struct TransparentNoteBuilder {
    address: Option<String>,
    txid: Option<TxId>,
    output_index: Option<u64>,
    script: Option<Vec<u8>>,
    value: Option<u64>,
    spent: Option<Option<(TxId, u32)>>,
    unconfirmed_spent: Option<Option<(TxId, u32)>>,
}
#[allow(dead_code)] //TODO:  fix this gross hack that I tossed in to silence the language-analyzer false positive
impl TransparentNoteBuilder {
    pub fn new() -> Self {
        Self::default()
    }
    // Methods to set each field
    build_method!(address, String);
    build_method!(txid, TxId);
    build_method!(output_index, u64);
    build_method!(script, Vec<u8>);
    build_method!(value, u64);
    build_method!(spent, Option<(TxId, u32)>);
    build_method!(unconfirmed_spent, Option<(TxId, u32)>);

    // Build method
    pub fn build(self) -> TransparentNote {
        TransparentNote::from_parts(
            self.address.unwrap(),
            self.txid.unwrap(),
            self.output_index.unwrap(),
            self.script.unwrap(),
            self.value.unwrap(),
            self.spent.unwrap(),
            self.unconfirmed_spent.unwrap(),
        )
    }
}

impl Default for TransparentNoteBuilder {
    fn default() -> Self {
        TransparentNoteBuilder {
            address: Some("default_address".to_string()),
            txid: Some(TxId::from_bytes([0u8; 32])),
            output_index: Some(0),
            script: Some(vec![]),
            value: Some(0),
            spent: Some(None),
            unconfirmed_spent: Some(None),
        }
    }
}

pub struct ProposalBuilder<FeeRuleT, NoteRef> {
    fee_rule: Option<FeeRuleT>,
    min_target_height: Option<BlockHeight>,
    steps: Option<NonEmpty<Step<NoteRef>>>,
}

impl<FeeRuleT, NoteRef> ProposalBuilder<FeeRuleT, NoteRef> {
    pub fn new() -> Self {
        ProposalBuilder {
            fee_rule: None,
            min_target_height: None,
            steps: None,
        }
    }

    build_method!(fee_rule, FeeRuleT);
    build_method!(min_target_height, BlockHeight);
    build_method!(steps, NonEmpty<Step<NoteRef>>);

    pub fn build(self) -> Proposal<FeeRuleT, NoteRef> {
        Proposal::single_step(
            self.steps.unwrap().first().transaction_request().clone(),
            self.steps.unwrap().first().payment_pools().clone(),
            self.steps.unwrap().first().transparent_inputs().to_vec(),
            self.steps.unwrap().first().shielded_inputs(),
            self.steps.unwrap().first().balance().clone(),
            self.fee_rule.unwrap(),
            self.min_target_height.unwrap(),
            self.steps.unwrap().first().is_shielding(),
        )
        .unwrap()
    }
}

pub struct StepBuilder<NoteRef> {
    transaction_request: Option<TransactionRequest>,
    payment_pools: Option<BTreeMap<usize, PoolType>>,
    transparent_inputs: Option<Vec<WalletTransparentOutput>>,
    shielded_inputs: Option<Option<ShieldedInputs<NoteRef>>>,
    prior_step_inputs: Option<Vec<StepOutput>>,
    balance: Option<TransactionBalance>,
    is_shielding: Option<bool>,
}

impl<NoteRef> StepBuilder<NoteRef> {
    pub fn new() -> Self {
        StepBuilder {
            transaction_request: None,
            payment_pools: None,
            transparent_inputs: None,
            shielded_inputs: None,
            prior_step_inputs: None,
            balance: None,
            is_shielding: None,
        }
    }

    build_method!(transaction_request, TransactionRequest);
    build_method!(payment_pools, BTreeMap<usize, PoolType>
    );
    build_method!(transparent_inputs, Vec<WalletTransparentOutput>);
    build_method!(shielded_inputs, Option<ShieldedInputs<NoteRef>>);
    build_method!(prior_step_inputs, Vec<StepOutput>);
    build_method!(balance, TransactionBalance);
    build_method!(is_shielding, bool);

    pub fn build(self) -> Step<NoteRef> {
        Step::from_parts(
            &[],
            self.transaction_request.unwrap(),
            self.payment_pools.unwrap(),
            self.transparent_inputs.unwrap(),
            self.shielded_inputs.unwrap(),
            self.prior_step_inputs.unwrap(),
            self.balance.unwrap(),
            self.is_shielding.unwrap(),
        )
        .unwrap()
    }
}

impl<NoteRef> Default for StepBuilder<NoteRef> {
    fn default() -> Self {
        Self::new()
            .transaction_request(TransactionRequest::empty())
            .payment_pools(BTreeMap::new())
            .transparent_inputs(vec![])
            .shielded_inputs(None)
            .prior_step_inputs(vec![])
            .balance(
                TransactionBalance::new(vec![], NonNegativeAmount::const_from_u64(20_000)).unwrap(),
            )
            .is_shielding(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_proposal() {
        let proposal = ProposalBuilder::new()
            .fee_rule(FeeRule::standard())
            .min_target_height(BlockHeight::from_u32(1))
            .steps(NonEmpty::singleton(StepBuilder::<()>::default().build()));
    }
}

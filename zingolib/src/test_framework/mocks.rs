//! Tools to facilitate mocks for testing

use std::collections::BTreeMap;

use incrementalmerkletree::Position;
use nonempty::NonEmpty;
use sapling_crypto::value::NoteValue;
use sapling_crypto::zip32::ExtendedSpendingKey;
use sapling_crypto::Rseed;
use zcash_client_backend::fees::TransactionBalance;
use zcash_client_backend::proposal::{Proposal, ShieldedInputs, Step, StepOutput};
use zcash_client_backend::wallet::{ReceivedNote, WalletTransparentOutput};
use zcash_client_backend::zip321::TransactionRequest;
use zcash_client_backend::PoolType;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::transaction::{
    components::amount::NonNegativeAmount, fees::zip317::FeeRule, TxId,
};

use crate::wallet::notes::{NoteRecordIdentifier, TransparentNote};

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

/// Provides a builder for constructing a mock [`zcash_client_backend::proposal::Proposal`].
///
/// # Examples
///
/// ```
/// use zingolib::test_framework::mocks::ProposalBuilder;
///
/// let proposal = ProposalBuilder::default().build();
/// ````
#[allow(dead_code)]
pub struct ProposalBuilder {
    fee_rule: Option<FeeRule>,
    min_target_height: Option<BlockHeight>,
    steps: Option<NonEmpty<Step<NoteRecordIdentifier>>>,
}

#[allow(dead_code)]
impl ProposalBuilder {
    /// Constructs a new [`ProposalBuilder`] with all fields as `None`.
    pub fn new() -> Self {
        ProposalBuilder {
            fee_rule: None,
            min_target_height: None,
            steps: None,
        }
    }

    build_method!(fee_rule, FeeRule);
    build_method!(min_target_height, BlockHeight);
    build_method!(steps, NonEmpty<Step<NoteRecordIdentifier>>);

    /// Builds a proposal after all fields have been set.
    ///
    /// # Panics
    ///
    /// `build` will panic if any fields of the builder are `None` or if the build failed
    /// due to invalid values.
    pub fn build(self) -> Proposal<FeeRule, NoteRecordIdentifier> {
        let step = self.steps.unwrap().first().clone();
        Proposal::single_step(
            step.transaction_request().clone(),
            step.payment_pools().clone(),
            step.transparent_inputs().to_vec(),
            step.shielded_inputs().cloned(),
            step.balance().clone(),
            self.fee_rule.unwrap(),
            self.min_target_height.unwrap(),
            step.is_shielding(),
        )
        .unwrap()
    }
}

impl Default for ProposalBuilder {
    /// Constructs a new [`ProposalBuilder`] where all fields are preset to default values.
    fn default() -> Self {
        ProposalBuilder::new()
            .fee_rule(FeeRule::standard())
            .min_target_height(BlockHeight::from_u32(1))
            .steps(NonEmpty::singleton(StepBuilder::default().build()))
    }
}

/// Provides a builder for constructing a mock [`zcash_client_backend::proposal::Step`].
///
/// # Examples
///
/// ```
/// use zingolib::test_framework::mocks::StepBuilder;
///
/// let step = StepBuilder::default().build();
/// ````
pub struct StepBuilder {
    transaction_request: Option<TransactionRequest>,
    payment_pools: Option<BTreeMap<usize, PoolType>>,
    transparent_inputs: Option<Vec<WalletTransparentOutput>>,
    shielded_inputs: Option<Option<ShieldedInputs<NoteRecordIdentifier>>>,
    prior_step_inputs: Option<Vec<StepOutput>>,
    balance: Option<TransactionBalance>,
    is_shielding: Option<bool>,
}

impl StepBuilder {
    /// Constructs a new [`StepBuilder`] with all fields as `None`.
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
    build_method!(
        shielded_inputs,
        Option<ShieldedInputs<NoteRecordIdentifier>>
    );
    build_method!(prior_step_inputs, Vec<StepOutput>);
    build_method!(balance, TransactionBalance);
    build_method!(is_shielding, bool);

    /// Builds a step after all fields have been set.
    ///
    /// # Panics
    ///
    /// `build` will panic if any fields of the builder are `None` or if the build failed
    /// due to invalid values.
    #[allow(dead_code)]
    pub fn build(self) -> Step<NoteRecordIdentifier> {
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

impl Default for StepBuilder {
    /// Constructs a new [`StepBuilder`] where all fields are preset to default values.
    fn default() -> Self {
        let txid = TxId::from_bytes([0u8; 32]);
        let seed = [0u8; 32];
        let dfvk = ExtendedSpendingKey::master(&seed).to_diversifiable_full_viewing_key();
        let (_, address) = dfvk.default_address();
        let note = sapling_crypto::Note::from_parts(
            address,
            NoteValue::from_raw(20_000),
            Rseed::AfterZip212([7; 32]),
        );

        Self::new()
            .transaction_request(TransactionRequest::empty())
            .payment_pools(BTreeMap::new())
            .transparent_inputs(vec![])
            // .shielded_inputs(None)
            .shielded_inputs(Some(ShieldedInputs::from_parts(
                BlockHeight::from_u32(1),
                NonEmpty::singleton(ReceivedNote::from_parts(
                    NoteRecordIdentifier {
                        txid,
                        pool: PoolType::Shielded(zcash_client_backend::ShieldedProtocol::Sapling),
                        index: 0,
                    },
                    txid,
                    0,
                    zcash_client_backend::wallet::Note::Sapling(note),
                    zip32::Scope::External,
                    Position::from(1),
                )),
            )))
            .prior_step_inputs(vec![])
            .balance(
                TransactionBalance::new(vec![], NonNegativeAmount::const_from_u64(20_000)).unwrap(),
            )
            .is_shielding(false)
    }
}

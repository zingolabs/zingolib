//! Tools to facilitate mocks for testing

macro_rules! build_method {
    ($name:ident, $localtype:ty) => {
        #[doc = "Set the $name field of the builder."]
        pub fn $name(mut self, $name: $localtype) -> Self {
            self.$name = Some($name);
            self
        }
    };
}
pub(crate) use build_method;
pub use proposal::{ProposalBuilder, StepBuilder};
pub use sapling_crypto_note::SaplingCryptoNoteBuilder;

fn zaddr_from_seed(
    seed: [u8; 32],
) -> (
    ExtendedSpendingKey,
    PreparedIncomingViewingKey,
    PaymentAddress,
) {
    let extsk = ExtendedSpendingKey::master(&seed);
    let dfvk = extsk.to_diversifiable_full_viewing_key();
    let fvk = dfvk;
    let (_, addr) = fvk.default_address();

    (
        extsk,
        PreparedIncomingViewingKey::new(&fvk.fvk().vk.ivk()),
        addr,
    )
}

/// This is the "all-0" base case!
pub fn default_txid() -> zcash_primitives::transaction::TxId {
    zcash_primitives::transaction::TxId::from_bytes([0u8; 32])
}
/// This is the "all-0" base case!
pub fn default_zaddr() -> (
    ExtendedSpendingKey,
    PreparedIncomingViewingKey,
    PaymentAddress,
) {
    zaddr_from_seed([0u8; 32])
}

use rand::{rngs::OsRng, Rng};
use sapling_crypto::{
    note_encryption::PreparedIncomingViewingKey, zip32::ExtendedSpendingKey, PaymentAddress,
};

/// Any old OS randomness
pub fn random_txid() -> zcash_primitives::transaction::TxId {
    let mut rng = OsRng;
    let mut seed = [0u8; 32];
    rng.fill(&mut seed);
    zcash_primitives::transaction::TxId::from_bytes(seed)
}
/// Any old OS randomness
pub fn random_zaddr() -> (
    ExtendedSpendingKey,
    PreparedIncomingViewingKey,
    PaymentAddress,
) {
    let mut rng = OsRng;
    let mut seed = [0u8; 32];
    rng.fill(&mut seed);

    zaddr_from_seed(seed)
}

// Sapling Note Mocker
mod sapling_crypto_note {

    use sapling_crypto::value::NoteValue;
    use sapling_crypto::Note;
    use sapling_crypto::PaymentAddress;
    use sapling_crypto::Rseed;

    use super::default_zaddr;

    /// A struct to build a mock sapling_crypto::Note from scratch.
    /// Distinguish [`sapling_crypto::Note`] from [`crate::wallet::notes::SaplingNote`]. The latter wraps the former with some other attributes.
    pub struct SaplingCryptoNoteBuilder {
        recipient: Option<PaymentAddress>,
        value: Option<NoteValue>,
        rseed: Option<Rseed>,
    }

    impl SaplingCryptoNoteBuilder {
        /// Instantiate an empty builder.
        pub fn new() -> Self {
            SaplingCryptoNoteBuilder {
                recipient: None,
                value: None,
                rseed: None,
            }
        }

        // Methods to set each field
        build_method!(recipient, PaymentAddress);
        build_method!(value, NoteValue);
        build_method!(rseed, Rseed);

        /// For any old zcaddr!
        pub fn randomize_recipient(self) -> Self {
            let (_, _, address) = super::random_zaddr();
            self.recipient(address)
        }

        /// Build the note.
        pub fn build(self) -> Note {
            Note::from_parts(
                self.recipient.unwrap(),
                self.value.unwrap(),
                self.rseed.unwrap(),
            )
        }
    }
    impl Default for SaplingCryptoNoteBuilder {
        fn default() -> Self {
            let (_, _, address) = default_zaddr();
            Self::new()
                .recipient(address)
                .value(NoteValue::from_raw(1000000))
                .rseed(Rseed::AfterZip212([7; 32]))
        }
    }
}

pub mod proposal {
    //! Module for mocking structs from [`zcash_client_backend::proposal`]

    use std::collections::BTreeMap;

    use incrementalmerkletree::Position;
    use nonempty::NonEmpty;
    use sapling_crypto::value::NoteValue;

    use sapling_crypto::Rseed;
    use zcash_client_backend::fees::TransactionBalance;
    use zcash_client_backend::proposal::{Proposal, ShieldedInputs, Step, StepOutput};
    use zcash_client_backend::wallet::{ReceivedNote, WalletTransparentOutput};
    use zcash_client_backend::zip321::TransactionRequest;
    use zcash_client_backend::PoolType;
    use zcash_primitives::consensus::BlockHeight;
    use zcash_primitives::transaction::{
        components::amount::NonNegativeAmount, fees::zip317::FeeRule,
    };

    use crate::wallet::notes::NoteRecordIdentifier;

    use super::{default_txid, default_zaddr};

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
            let txid = default_txid();
            let (_, _, address) = default_zaddr();
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
                            pool: PoolType::Shielded(
                                zcash_client_backend::ShieldedProtocol::Sapling,
                            ),
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
                    TransactionBalance::new(vec![], NonNegativeAmount::const_from_u64(20_000))
                        .unwrap(),
                )
                .is_shielding(false)
        }
    }
}

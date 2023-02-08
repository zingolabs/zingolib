//! Provides unifying interfaces for transaction management across Sapling and Orchard
use std::io::{self, Read, Write};

use super::{
    data::{
        PoolNullifier, ReceivedOrchardNoteAndMetadata, ReceivedSaplingNoteAndMetadata,
        SpendableOrchardNote, SpendableSaplingNote, TransactionMetadata, WitnessCache,
    },
    keys::unified::UnifiedSpendCapability,
    transactions::TransactionMetadataSet,
};
use crate::compact_formats::{
    vec_to_array, CompactOrchardAction, CompactSaplingOutput, CompactTx, TreeState,
};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use nonempty::NonEmpty;
use orchard::{
    bundle::{Authorized as OrchardAuthorized, Bundle as OrchardBundle},
    keys::{
        Diversifier as OrchardDiversifier, FullViewingKey as OrchardFullViewingKey,
        SpendingKey as OrchardSpendingKey,
    },
    note::{Note as OrchardNote, Nullifier as OrchardNullifier},
    note_encryption::OrchardDomain,
    primitives::redpallas::{Signature, SpendAuth},
    tree::MerkleHashOrchard,
    Action, Address as OrchardAddress,
};
use subtle::CtOption;
use zcash_address::unified::{self, Encoding as _, Receiver};
use zcash_client_backend::{address::UnifiedAddress, encoding::encode_payment_address};
use zcash_encoding::{Optional, Vector};
use zcash_note_encryption::{
    BatchDomain, Domain, ShieldedOutput, COMPACT_NOTE_SIZE, ENC_CIPHERTEXT_SIZE,
};
use zcash_primitives::{
    consensus::{BlockHeight, NetworkUpgrade, Parameters},
    memo::{Memo, MemoBytes},
    merkle_tree::{Hashable, IncrementalWitness},
    sapling::{
        note_encryption::SaplingDomain, Diversifier as SaplingDiversifier, Node as SaplingNode,
        Note as SaplingNote, Nullifier as SaplingNullifier, PaymentAddress as SaplingAddress,
    },
    transaction::{
        components::{
            sapling::{
                Authorization as SaplingAuthorization, Authorized as SaplingAuthorized,
                Bundle as SaplingBundle, GrothProofBytes,
            },
            Amount, OutputDescription, SpendDescription,
        },
        Transaction, TxId,
    },
    zip32::{
        ExtendedFullViewingKey as SaplingExtendedFullViewingKey,
        ExtendedSpendingKey as SaplingExtendedSpendingKey,
    },
};
use zingoconfig::ChainType;

/// This provides a uniform `.to_bytes` to types that might require it in a generic context.
pub trait ToBytes<const N: usize> {
    fn to_bytes(&self) -> [u8; N];
}

impl ToBytes<32> for SaplingNullifier {
    fn to_bytes(&self) -> [u8; 32] {
        self.0
    }
}

impl ToBytes<32> for OrchardNullifier {
    fn to_bytes(&self) -> [u8; 32] {
        OrchardNullifier::to_bytes(*self)
    }
}

impl ToBytes<11> for SaplingDiversifier {
    fn to_bytes(&self) -> [u8; 11] {
        self.0
    }
}

impl ToBytes<11> for OrchardDiversifier {
    fn to_bytes(&self) -> [u8; 11] {
        *self.as_array()
    }
}

impl ToBytes<512> for Memo {
    fn to_bytes(&self) -> [u8; 512] {
        *self.encode().as_array()
    }
}

impl ToBytes<512> for MemoBytes {
    fn to_bytes(&self) -> [u8; 512] {
        *self.as_array()
    }
}

impl<const N: usize> ToBytes<N> for [u8; N] {
    fn to_bytes(&self) -> [u8; N] {
        *self
    }
}

/// Exposes the out_ciphertext, domain, and value_commitment in addition to the
/// required methods of ShieldedOutput
pub trait ShieldedOutputExt<D: Domain>: ShieldedOutput<D, ENC_CIPHERTEXT_SIZE> {
    fn domain(&self, height: BlockHeight, parameters: ChainType) -> D;
    /// A decryption key for `enc_ciphertext`.  `out_ciphertext` is _itself_  decryptable
    /// with the `OutgoingCipherKey` "`ock`".
    fn out_ciphertext(&self) -> [u8; 80];
    fn value_commitment(&self) -> D::ValueCommitment;
}

impl<A> ShieldedOutputExt<OrchardDomain> for Action<A> {
    fn domain(&self, _block_height: BlockHeight, _parameters: ChainType) -> OrchardDomain {
        OrchardDomain::for_action(self)
    }

    fn out_ciphertext(&self) -> [u8; 80] {
        self.encrypted_note().out_ciphertext
    }

    fn value_commitment(&self) -> orchard::value::ValueCommitment {
        self.cv_net().clone()
    }
}

impl ShieldedOutputExt<SaplingDomain<ChainType>> for OutputDescription<GrothProofBytes> {
    fn domain(&self, height: BlockHeight, parameters: ChainType) -> SaplingDomain<ChainType> {
        SaplingDomain::for_height(parameters, height)
    }

    fn out_ciphertext(&self) -> [u8; 80] {
        self.out_ciphertext
    }

    fn value_commitment(&self) -> <SaplingDomain<ChainType> as Domain>::ValueCommitment {
        self.cv
    }
}

/// Provides a standard `from_bytes` interface to be used generically
pub trait FromBytes<const N: usize> {
    fn from_bytes(bytes: [u8; N]) -> Self;
}

impl FromBytes<32> for SaplingNullifier {
    fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl FromBytes<32> for OrchardNullifier {
    fn from_bytes(bytes: [u8; 32]) -> Self {
        Option::from(OrchardNullifier::from_bytes(&bytes))
            .expect(&format!("Invalid nullifier {:?}", bytes))
    }
}

impl FromBytes<11> for SaplingDiversifier {
    fn from_bytes(bytes: [u8; 11]) -> Self {
        SaplingDiversifier(bytes)
    }
}

impl FromBytes<11> for OrchardDiversifier {
    fn from_bytes(bytes: [u8; 11]) -> Self {
        OrchardDiversifier::from_bytes(bytes)
    }
}

pub trait FromCommitment
where
    Self: Sized,
{
    fn from_commitment(from: &[u8; 32]) -> CtOption<Self>;
}

impl FromCommitment for SaplingNode {
    fn from_commitment(from: &[u8; 32]) -> CtOption<Self> {
        CtOption::new(Self::new(*from), subtle::Choice::from(1))
    }
}
impl FromCommitment for MerkleHashOrchard {
    fn from_commitment(from: &[u8; 32]) -> CtOption<Self> {
        Self::from_bytes(from)
    }
}

/// The component that transfers value.  In the common case, from one output to another.
pub trait Spend {
    type Nullifier: Nullifier;
    fn nullifier(&self) -> &Self::Nullifier;
}

impl<Auth: SaplingAuthorization> Spend for SpendDescription<Auth> {
    type Nullifier = SaplingNullifier;
    fn nullifier(&self) -> &Self::Nullifier {
        &self.nullifier
    }
}

impl<Auth> Spend for Action<Auth> {
    type Nullifier = OrchardNullifier;
    fn nullifier(&self) -> &Self::Nullifier {
        self.nullifier()
    }
}

impl From<OrchardNullifier> for PoolNullifier {
    fn from(n: OrchardNullifier) -> Self {
        PoolNullifier::Orchard(n)
    }
}

impl From<SaplingNullifier> for PoolNullifier {
    fn from(n: SaplingNullifier) -> Self {
        PoolNullifier::Sapling(n)
    }
}

///  Recipients provide the means to generate a Receiver.  A Receiver contains the information necessary
///  to transfer an asset to the generating Recipient.
///  <https://zips.z.cash/zip-0316#terminology>
pub trait Recipient {
    type Diversifier: Copy;
    fn diversifier(&self) -> Self::Diversifier;
    fn b32encode_for_network(&self, chain: &ChainType) -> String;
}

impl Recipient for OrchardAddress {
    type Diversifier = OrchardDiversifier;

    fn diversifier(&self) -> Self::Diversifier {
        OrchardAddress::diversifier(&self)
    }

    fn b32encode_for_network(&self, chain: &ChainType) -> String {
        unified::Address::try_from_items(vec![Receiver::Orchard(self.to_raw_address_bytes())])
            .expect("Could not create UA from orchard address")
            .encode(&chain.address_network().unwrap())
    }
}

impl Recipient for SaplingAddress {
    type Diversifier = SaplingDiversifier;

    fn diversifier(&self) -> Self::Diversifier {
        *SaplingAddress::diversifier(&self)
    }

    fn b32encode_for_network(&self, chain: &ChainType) -> String {
        encode_payment_address(chain.hrp_sapling_payment_address(), self)
    }
}

pub trait CompactOutput<D: DomainWalletExt>:
    Sized + ShieldedOutput<D, COMPACT_NOTE_SIZE> + Clone
where
    D::Recipient: Recipient,
    <D as Domain>::Note: PartialEq + Clone,
{
    fn from_compact_transaction(compact_transaction: &CompactTx) -> &Vec<Self>;
    fn cmstar(&self) -> &[u8; 32];
    fn domain(&self, parameters: ChainType, height: BlockHeight) -> D;
}

impl CompactOutput<SaplingDomain<ChainType>> for CompactSaplingOutput {
    fn from_compact_transaction(compact_transaction: &CompactTx) -> &Vec<CompactSaplingOutput> {
        &compact_transaction.outputs
    }

    fn cmstar(&self) -> &[u8; 32] {
        vec_to_array(&self.cmu)
    }

    fn domain(&self, parameters: ChainType, height: BlockHeight) -> SaplingDomain<ChainType> {
        SaplingDomain::for_height(parameters, height)
    }
}

impl CompactOutput<OrchardDomain> for CompactOrchardAction {
    fn from_compact_transaction(compact_transaction: &CompactTx) -> &Vec<CompactOrchardAction> {
        &compact_transaction.actions
    }
    fn cmstar(&self) -> &[u8; 32] {
        vec_to_array(&self.cmx)
    }

    fn domain(&self, _parameters: ChainType, _heightt: BlockHeight) -> OrchardDomain {
        OrchardDomain::for_nullifier(
            OrchardNullifier::from_bytes(vec_to_array(&self.nullifier)).unwrap(),
        )
    }
}

/// A set of transmission abstractions within a transaction, that are specific to a particular
/// domain. In the Orchard Domain bundles comprise Actions each of which contains
/// both a Spend and an Output (though either or both may be dummies). Sapling transmissions,
/// as implemented, contain a 1:1 ratio of Spends and Outputs.
pub trait Bundle<D: DomainWalletExt>
where
    D::Recipient: Recipient,
    D::Note: PartialEq + Clone,
{
    /// An expenditure of an output, such that its value is distributed among *this* transaction's outputs.
    type Spend: Spend;
    /// A value store that is completely emptied by transfer of its contents to another output.
    type Output: ShieldedOutputExt<D> + Clone;
    type Spends<'a>: IntoIterator<Item = &'a Self::Spend>
    where
        Self::Spend: 'a,
        Self: 'a;
    type Outputs<'a>: IntoIterator<Item = &'a Self::Output>
    where
        Self::Output: 'a,
        Self: 'a;
    /// An extractive process that returns domain specific information from a transaction.
    fn from_transaction(transaction: &Transaction) -> Option<&Self>;
    /// Some domains, Orchard for example, do not expose
    /// immediately expose outputs
    fn output_elements(&self) -> Self::Outputs<'_>;
    fn spend_elements(&self) -> Self::Spends<'_>;
}

impl Bundle<SaplingDomain<ChainType>> for SaplingBundle<SaplingAuthorized> {
    type Spend = SpendDescription<SaplingAuthorized>;
    type Output = OutputDescription<GrothProofBytes>;
    type Spends<'a> = &'a Vec<Self::Spend>;
    type Outputs<'a> = &'a Vec<Self::Output>;
    fn from_transaction(transaction: &Transaction) -> Option<&Self> {
        transaction.sapling_bundle()
    }

    fn output_elements(&self) -> Self::Outputs<'_> {
        &self.shielded_outputs
    }

    fn spend_elements(&self) -> Self::Spends<'_> {
        &self.shielded_spends
    }
}

impl Bundle<OrchardDomain> for OrchardBundle<OrchardAuthorized, Amount> {
    type Spend = Action<Signature<SpendAuth>>;
    type Output = Action<Signature<SpendAuth>>;
    type Spends<'a> = &'a NonEmpty<Self::Spend>;
    type Outputs<'a> = &'a NonEmpty<Self::Output>;

    fn from_transaction(transaction: &Transaction) -> Option<&Self> {
        transaction.orchard_bundle()
    }

    fn output_elements(&self) -> Self::Outputs<'_> {
        //! In orchard each action contains an output and a spend.
        self.actions()
    }

    fn spend_elements(&self) -> Self::Spends<'_> {
        //! In orchard each action contains an output and a spend.
        self.actions()
    }
}

/// TODO: Documentation neeeeeds help!!!!  XXXX
pub trait Nullifier:
    PartialEq + Copy + Sized + ToBytes<32> + FromBytes<32> + Send + Into<PoolNullifier>
{
    fn get_nullifiers_of_unspent_notes_from_transaction_set(
        transaction_metadata_set: &TransactionMetadataSet,
    ) -> Vec<(Self, u64, TxId)>;
    fn get_nullifiers_spent_in_transaction(transaction: &TransactionMetadata) -> &Vec<Self>;
}

impl Nullifier for SaplingNullifier {
    fn get_nullifiers_of_unspent_notes_from_transaction_set(
        transaction_metadata_set: &TransactionMetadataSet,
    ) -> Vec<(Self, u64, TxId)> {
        transaction_metadata_set.get_nullifiers_of_unspent_sapling_notes()
    }

    fn get_nullifiers_spent_in_transaction(
        transaction_metadata_set: &TransactionMetadata,
    ) -> &Vec<Self> {
        &transaction_metadata_set.spent_sapling_nullifiers
    }
}

impl Nullifier for OrchardNullifier {
    fn get_nullifiers_of_unspent_notes_from_transaction_set(
        transactions: &TransactionMetadataSet,
    ) -> Vec<(Self, u64, TxId)> {
        transactions.get_nullifiers_of_unspent_orchard_notes()
    }

    fn get_nullifiers_spent_in_transaction(transaction: &TransactionMetadata) -> &Vec<Self> {
        &transaction.spent_orchard_nullifiers
    }
}

pub trait ReceivedNoteAndMetadata: Sized {
    type Fvk: Clone + Diversifiable + ReadableWriteable<()> + Send;
    type Diversifier: Copy + FromBytes<11> + ToBytes<11>;
    type Note: PartialEq + ReadableWriteable<(Self::Fvk, Self::Diversifier)> + Clone;
    type Node: Hashable + FromCommitment + Send;
    type Nullifier: Nullifier;
    const GET_NOTE_WITNESSES: fn(
        &TransactionMetadataSet,
        &TxId,
        &Self::Nullifier,
    ) -> Option<(WitnessCache<Self::Node>, BlockHeight)>;
    const SET_NOTE_WITNESSES: fn(
        &mut TransactionMetadataSet,
        &TxId,
        &Self::Nullifier,
        WitnessCache<Self::Node>,
    );
    fn from_parts(
        extfvk: Self::Fvk,
        diversifier: Self::Diversifier,
        note: Self::Note,
        witnesses: WitnessCache<Self::Node>,
        nullifier: Self::Nullifier,
        spent: Option<(TxId, u32)>,
        unconfirmed_spent: Option<(TxId, u32)>,
        memo: Option<Memo>,
        is_change: bool,
        have_spending_key: bool,
    ) -> Self;
    fn is_change(&self) -> bool;
    fn fvk(&self) -> &Self::Fvk;
    fn diversifier(
        &self,
    ) -> &<<Self::Fvk as Diversifiable>::Note as ReceivedNoteAndMetadata>::Diversifier;
    fn memo(&self) -> &Option<Memo>;
    fn memo_mut(&mut self) -> &mut Option<Memo>;
    fn note(&self) -> &Self::Note;
    fn get_nullifier_from_note_fvk_and_witness_position(
        note: &Self::Note,
        fvk: &Self::Fvk,
        position: u64,
    ) -> Self::Nullifier;
    fn nullifier(&self) -> Self::Nullifier;
    fn value_from_note(note: &Self::Note) -> u64;
    fn spent(&self) -> &Option<(TxId, u32)>;
    fn spent_mut(&mut self) -> &mut Option<(TxId, u32)>;
    fn unconfirmed_spent(&self) -> &Option<(TxId, u32)>;
    fn unconfirmed_spent_mut(&mut self) -> &mut Option<(TxId, u32)>;
    fn witnesses(&self) -> &WitnessCache<Self::Node>;
    fn witnesses_mut(&mut self) -> &mut WitnessCache<Self::Node>;
    fn have_spending_key(&self) -> bool;
    fn transaction_metadata_notes(wallet_transaction: &TransactionMetadata) -> &Vec<Self>;
    fn transaction_metadata_notes_mut(
        wallet_transaction: &mut TransactionMetadata,
    ) -> &mut Vec<Self>;
    ///Convenience function
    fn value(&self) -> u64 {
        Self::value_from_note(self.note())
    }
    fn is_spent(&self) -> bool {
        Self::spent(self).is_some()
    }
}

impl ReceivedNoteAndMetadata for ReceivedSaplingNoteAndMetadata {
    type Fvk = SaplingExtendedFullViewingKey;
    type Diversifier = SaplingDiversifier;
    type Note = SaplingNote;
    type Node = SaplingNode;
    type Nullifier = SaplingNullifier;

    /// This (and SET_NOTE_WITNESSES) could be associated functions instead of fn
    /// constants, but that would require an additional repetition of the fn arguments.
    const GET_NOTE_WITNESSES: fn(
        &TransactionMetadataSet,
        &TxId,
        &Self::Nullifier,
    ) -> Option<(WitnessCache<Self::Node>, BlockHeight)> =
        TransactionMetadataSet::get_sapling_note_witnesses;

    const SET_NOTE_WITNESSES: fn(
        &mut TransactionMetadataSet,
        &TxId,
        &Self::Nullifier,
        WitnessCache<Self::Node>,
    ) = TransactionMetadataSet::set_sapling_note_witnesses;

    fn from_parts(
        extfvk: SaplingExtendedFullViewingKey,
        diversifier: SaplingDiversifier,
        note: SaplingNote,
        witnesses: WitnessCache<SaplingNode>,
        nullifier: SaplingNullifier,
        spent: Option<(TxId, u32)>,
        unconfirmed_spent: Option<(TxId, u32)>,
        memo: Option<Memo>,
        is_change: bool,
        have_spending_key: bool,
    ) -> Self {
        Self {
            extfvk,
            diversifier,
            note,
            witnesses,
            nullifier,
            spent,
            unconfirmed_spent,
            memo,
            is_change,
            have_spending_key,
        }
    }

    fn is_change(&self) -> bool {
        self.is_change
    }

    fn fvk(&self) -> &Self::Fvk {
        &self.extfvk
    }

    fn diversifier(&self) -> &Self::Diversifier {
        &self.diversifier
    }

    fn memo(&self) -> &Option<Memo> {
        &self.memo
    }

    fn memo_mut(&mut self) -> &mut Option<Memo> {
        &mut self.memo
    }

    fn note(&self) -> &Self::Note {
        &self.note
    }

    fn get_nullifier_from_note_fvk_and_witness_position(
        note: &Self::Note,
        extfvk: &Self::Fvk,
        position: u64,
    ) -> Self::Nullifier {
        note.nf(&extfvk.fvk.vk.nk, position)
    }

    fn nullifier(&self) -> Self::Nullifier {
        self.nullifier
    }

    fn value_from_note(note: &Self::Note) -> u64 {
        note.value
    }

    fn spent(&self) -> &Option<(TxId, u32)> {
        &self.spent
    }

    fn spent_mut(&mut self) -> &mut Option<(TxId, u32)> {
        &mut self.spent
    }

    fn unconfirmed_spent(&self) -> &Option<(TxId, u32)> {
        &self.unconfirmed_spent
    }

    fn unconfirmed_spent_mut(&mut self) -> &mut Option<(TxId, u32)> {
        &mut self.unconfirmed_spent
    }

    fn witnesses(&self) -> &WitnessCache<Self::Node> {
        &self.witnesses
    }

    fn witnesses_mut(&mut self) -> &mut WitnessCache<Self::Node> {
        &mut self.witnesses
    }

    fn have_spending_key(&self) -> bool {
        self.have_spending_key
    }

    fn transaction_metadata_notes(wallet_transaction: &TransactionMetadata) -> &Vec<Self> {
        &wallet_transaction.sapling_notes
    }

    fn transaction_metadata_notes_mut(
        wallet_transaction: &mut TransactionMetadata,
    ) -> &mut Vec<Self> {
        &mut wallet_transaction.sapling_notes
    }
}

impl ReceivedNoteAndMetadata for ReceivedOrchardNoteAndMetadata {
    type Fvk = OrchardFullViewingKey;
    type Diversifier = OrchardDiversifier;
    type Note = OrchardNote;
    type Node = MerkleHashOrchard;
    type Nullifier = OrchardNullifier;

    const GET_NOTE_WITNESSES: fn(
        &TransactionMetadataSet,
        &TxId,
        &Self::Nullifier,
    ) -> Option<(WitnessCache<Self::Node>, BlockHeight)> =
        TransactionMetadataSet::get_orchard_note_witnesses;

    const SET_NOTE_WITNESSES: fn(
        &mut TransactionMetadataSet,
        &TxId,
        &Self::Nullifier,
        WitnessCache<Self::Node>,
    ) = TransactionMetadataSet::set_orchard_note_witnesses;

    fn from_parts(
        fvk: Self::Fvk,
        diversifier: Self::Diversifier,
        note: Self::Note,
        witnesses: WitnessCache<Self::Node>,
        nullifier: Self::Nullifier,
        spent: Option<(TxId, u32)>,
        unconfirmed_spent: Option<(TxId, u32)>,
        memo: Option<Memo>,
        is_change: bool,
        have_spending_key: bool,
    ) -> Self {
        Self {
            fvk,
            diversifier,
            note,
            witnesses,
            nullifier,
            spent,
            unconfirmed_spent,
            memo,
            is_change,
            have_spending_key,
        }
    }

    fn is_change(&self) -> bool {
        self.is_change
    }
    fn fvk(&self) -> &Self::Fvk {
        &self.fvk
    }
    fn diversifier(&self) -> &Self::Diversifier {
        &self.diversifier
    }
    fn memo(&self) -> &Option<Memo> {
        &self.memo
    }
    fn memo_mut(&mut self) -> &mut Option<Memo> {
        &mut self.memo
    }

    fn note(&self) -> &Self::Note {
        &self.note
    }

    fn get_nullifier_from_note_fvk_and_witness_position(
        note: &Self::Note,
        fvk: &Self::Fvk,
        _position: u64,
    ) -> Self::Nullifier {
        note.nullifier(fvk)
    }

    fn nullifier(&self) -> Self::Nullifier {
        self.nullifier
    }

    fn value_from_note(note: &Self::Note) -> u64 {
        note.value().inner()
    }

    fn spent(&self) -> &Option<(TxId, u32)> {
        &self.spent
    }

    fn spent_mut(&mut self) -> &mut Option<(TxId, u32)> {
        &mut self.spent
    }

    fn unconfirmed_spent(&self) -> &Option<(TxId, u32)> {
        &self.unconfirmed_spent
    }

    fn unconfirmed_spent_mut(&mut self) -> &mut Option<(TxId, u32)> {
        &mut self.unconfirmed_spent
    }

    fn witnesses(&self) -> &WitnessCache<Self::Node> {
        &self.witnesses
    }

    fn witnesses_mut(&mut self) -> &mut WitnessCache<Self::Node> {
        &mut self.witnesses
    }

    fn have_spending_key(&self) -> bool {
        self.have_spending_key
    }

    fn transaction_metadata_notes(wallet_transaction: &TransactionMetadata) -> &Vec<Self> {
        &wallet_transaction.orchard_notes
    }

    fn transaction_metadata_notes_mut(
        wallet_transaction: &mut TransactionMetadata,
    ) -> &mut Vec<Self> {
        &mut wallet_transaction.orchard_notes
    }
}

pub trait DomainWalletExt: Domain + BatchDomain
where
    Self: Sized,
    Self::Note: PartialEq + Clone,
    Self::Recipient: Recipient,
{
    const NU: NetworkUpgrade;

    type Fvk: Clone
        + Send
        + Diversifiable<Note = Self::WalletNote, Address = Self::Recipient>
        + PartialEq;

    type SpendingKey: for<'a> From<&'a UnifiedSpendCapability> + Clone;
    type CompactOutput: CompactOutput<Self>;
    type WalletNote: ReceivedNoteAndMetadata<
        Fvk = Self::Fvk,
        Note = <Self as Domain>::Note,
        Diversifier = <<Self as Domain>::Recipient as Recipient>::Diversifier,
        Nullifier = <<<Self as DomainWalletExt>::Bundle as Bundle<Self>>::Spend as Spend>::Nullifier,
    >;
    type SpendableNoteAT: SpendableNote<Self>;

    type Bundle: Bundle<Self>;

    fn to_notes_vec_mut(_: &mut TransactionMetadata) -> &mut Vec<Self::WalletNote>;
    fn ua_from_contained_receiver<'a>(
        unified_spend_auth: &'a UnifiedSpendCapability,
        receiver: &Self::Recipient,
    ) -> Option<&'a UnifiedAddress>;
    fn get_tree(tree_state: &TreeState) -> &String;
    fn usc_to_sk(usc: &UnifiedSpendCapability) -> Self::SpendingKey;
    fn usc_to_fvk(usc: &UnifiedSpendCapability) -> Self::Fvk;
    fn usc_to_ivk(usc: &UnifiedSpendCapability) -> Self::IncomingViewingKey;
    fn usc_to_ovk(usc: &UnifiedSpendCapability) -> Self::OutgoingViewingKey;
}

impl DomainWalletExt for SaplingDomain<ChainType> {
    const NU: NetworkUpgrade = NetworkUpgrade::Sapling;

    type Fvk = SaplingExtendedFullViewingKey;

    type SpendingKey = SaplingExtendedSpendingKey;

    type CompactOutput = CompactSaplingOutput;

    type WalletNote = ReceivedSaplingNoteAndMetadata;

    type SpendableNoteAT = SpendableSaplingNote;

    type Bundle = SaplingBundle<SaplingAuthorized>;

    fn to_notes_vec_mut(transaction: &mut TransactionMetadata) -> &mut Vec<Self::WalletNote> {
        &mut transaction.sapling_notes
    }

    fn ua_from_contained_receiver<'a>(
        unified_spend_auth: &'a UnifiedSpendCapability,
        receiver: &Self::Recipient,
    ) -> Option<&'a UnifiedAddress> {
        unified_spend_auth
            .addresses()
            .iter()
            .find(|ua| ua.sapling() == Some(receiver))
    }

    fn get_tree(tree_state: &TreeState) -> &String {
        &tree_state.sapling_tree
    }
    fn usc_to_sk(usc: &UnifiedSpendCapability) -> Self::SpendingKey {
        Self::SpendingKey::from(usc)
    }
    fn usc_to_fvk(usc: &UnifiedSpendCapability) -> Self::Fvk {
        Self::Fvk::from(usc)
    }
    fn usc_to_ivk(usc: &UnifiedSpendCapability) -> Self::IncomingViewingKey {
        Self::IncomingViewingKey::from(usc)
    }
    fn usc_to_ovk(usc: &UnifiedSpendCapability) -> Self::OutgoingViewingKey {
        Self::OutgoingViewingKey::from(usc)
    }
}

impl DomainWalletExt for OrchardDomain {
    const NU: NetworkUpgrade = NetworkUpgrade::Nu5;

    type Fvk = OrchardFullViewingKey;

    type SpendingKey = OrchardSpendingKey;

    type CompactOutput = CompactOrchardAction;

    type WalletNote = ReceivedOrchardNoteAndMetadata;

    type SpendableNoteAT = SpendableOrchardNote;

    type Bundle = OrchardBundle<OrchardAuthorized, Amount>;

    fn to_notes_vec_mut(transaction: &mut TransactionMetadata) -> &mut Vec<Self::WalletNote> {
        &mut transaction.orchard_notes
    }

    fn ua_from_contained_receiver<'a>(
        unified_spend_capability: &'a UnifiedSpendCapability,
        receiver: &Self::Recipient,
    ) -> Option<&'a UnifiedAddress> {
        unified_spend_capability
            .addresses()
            .iter()
            .find(|unified_address| unified_address.orchard() == Some(receiver))
    }

    fn get_tree(tree_state: &TreeState) -> &String {
        &tree_state.orchard_tree
    }
    fn usc_to_sk(usc: &UnifiedSpendCapability) -> Self::SpendingKey {
        Self::SpendingKey::from(usc)
    }
    fn usc_to_fvk(usc: &UnifiedSpendCapability) -> Self::Fvk {
        Self::Fvk::from(usc)
    }
    fn usc_to_ivk(usc: &UnifiedSpendCapability) -> Self::IncomingViewingKey {
        Self::IncomingViewingKey::from(usc)
    }
    fn usc_to_ovk(usc: &UnifiedSpendCapability) -> Self::OutgoingViewingKey {
        Self::OutgoingViewingKey::from(usc)
    }
}

pub trait Diversifiable {
    type Note: ReceivedNoteAndMetadata;
    type Address: Recipient;
    fn diversified_address(
        &self,
        div: <Self::Note as ReceivedNoteAndMetadata>::Diversifier,
    ) -> Option<Self::Address>;
}

impl Diversifiable for SaplingExtendedFullViewingKey {
    type Note = ReceivedSaplingNoteAndMetadata;

    type Address = zcash_primitives::sapling::PaymentAddress;

    fn diversified_address(
        &self,
        div: <<zcash_primitives::zip32::ExtendedFullViewingKey as Diversifiable>::Note as ReceivedNoteAndMetadata>::Diversifier,
    ) -> Option<Self::Address> {
        self.fvk.vk.to_payment_address(div)
    }
}

impl Diversifiable for OrchardFullViewingKey {
    type Note = ReceivedOrchardNoteAndMetadata;
    type Address = orchard::Address;

    fn diversified_address(
        &self,
        div: <<orchard::keys::FullViewingKey as Diversifiable>::Note as ReceivedNoteAndMetadata>::Diversifier,
    ) -> Option<Self::Address> {
        Some(self.address(div, orchard::keys::Scope::External))
    }
}

pub trait SpendableNote<D>
where
    D: DomainWalletExt<SpendableNoteAT = Self>,
    <D as Domain>::Recipient: Recipient,
    <D as Domain>::Note: PartialEq + Clone,
    Self: Sized,
{
    fn from(
        transaction_id: TxId,
        note_and_metadata: &D::WalletNote,
        anchor_offset: usize,
        spend_key: Option<&D::SpendingKey>,
    ) -> Option<Self> {
        // Include only notes that haven't been spent, or haven't been included in an unconfirmed spend yet.
        if note_and_metadata.spent().is_none()
            && note_and_metadata.unconfirmed_spent().is_none()
            && spend_key.is_some()
            && note_and_metadata.witnesses().len() >= (anchor_offset + 1)
        {
            let witness = note_and_metadata
                .witnesses()
                .get(note_and_metadata.witnesses().len() - anchor_offset - 1);

            witness.map(|w| {
                Self::from_parts_unchecked(
                    transaction_id,
                    note_and_metadata.nullifier(),
                    *note_and_metadata.diversifier(),
                    note_and_metadata.note().clone(),
                    w.clone(),
                    spend_key,
                )
            })
        } else {
            None
        }
    }
    /// The checks needed are shared between domains, and thus are performed in the
    /// default impl of `from`. This function's only caller should be `Self::from`
    fn from_parts_unchecked(
        transaction_id: TxId,
        nullifier: <D::WalletNote as ReceivedNoteAndMetadata>::Nullifier,
        diversifier: <D::WalletNote as ReceivedNoteAndMetadata>::Diversifier,
        note: D::Note,
        witness: IncrementalWitness<<D::WalletNote as ReceivedNoteAndMetadata>::Node>,
        sk: Option<&D::SpendingKey>,
    ) -> Self;
    fn transaction_id(&self) -> TxId;
    fn nullifier(&self) -> <D::WalletNote as ReceivedNoteAndMetadata>::Nullifier;
    fn diversifier(&self) -> <D::WalletNote as ReceivedNoteAndMetadata>::Diversifier;
    fn note(&self) -> &D::Note;
    fn witness(&self) -> &IncrementalWitness<<D::WalletNote as ReceivedNoteAndMetadata>::Node>;
    fn spend_key(&self) -> Option<&D::SpendingKey>;
}

impl SpendableNote<SaplingDomain<ChainType>> for SpendableSaplingNote {
    fn from_parts_unchecked(
        transaction_id: TxId,
        nullifier: SaplingNullifier,
        diversifier: SaplingDiversifier,
        note: SaplingNote,
        witness: IncrementalWitness<SaplingNode>,
        extsk: Option<&SaplingExtendedSpendingKey>,
    ) -> Self {
        SpendableSaplingNote {
            transaction_id,
            nullifier,
            diversifier,
            note,
            witness,
            extsk: extsk.cloned(),
        }
    }

    fn transaction_id(&self) -> TxId {
        self.transaction_id
    }

    fn nullifier(&self) -> SaplingNullifier {
        self.nullifier
    }

    fn diversifier(&self) -> SaplingDiversifier {
        self.diversifier
    }

    fn note(&self) -> &SaplingNote {
        &self.note
    }

    fn witness(&self) -> &IncrementalWitness<SaplingNode> {
        &self.witness
    }

    fn spend_key(&self) -> Option<&SaplingExtendedSpendingKey> {
        self.extsk.as_ref()
    }
}

impl SpendableNote<OrchardDomain> for SpendableOrchardNote {
    fn from_parts_unchecked(
        transaction_id: TxId,
        nullifier: OrchardNullifier,
        diversifier: OrchardDiversifier,
        note: OrchardNote,
        witness: IncrementalWitness<MerkleHashOrchard>,
        sk: Option<&OrchardSpendingKey>,
    ) -> Self {
        SpendableOrchardNote {
            transaction_id,
            nullifier,
            diversifier,
            note,
            witness,
            spend_key: sk.cloned(),
        }
    }
    fn transaction_id(&self) -> TxId {
        self.transaction_id
    }

    fn nullifier(&self) -> OrchardNullifier {
        self.nullifier
    }

    fn diversifier(&self) -> OrchardDiversifier {
        self.diversifier
    }

    fn note(&self) -> &orchard::Note {
        &self.note
    }

    fn witness(&self) -> &IncrementalWitness<MerkleHashOrchard> {
        &self.witness
    }

    fn spend_key(&self) -> Option<&OrchardSpendingKey> {
        self.spend_key.as_ref()
    }
}

pub trait ReadableWriteable<Input>: Sized {
    const VERSION: u8;

    fn read<R: Read>(reader: R, input: Input) -> io::Result<Self>;
    fn write<W: Write>(&self, writer: W) -> io::Result<()>;
    fn get_version<R: Read>(mut reader: R) -> io::Result<u8> {
        let version = reader.read_u8()?;
        if version > Self::VERSION {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Wallet file version \"{}\" is from future version of zingo",
                    version,
                ),
            ))
        } else {
            Ok(version)
        }
    }
}

impl ReadableWriteable<()> for SaplingExtendedFullViewingKey {
    const VERSION: u8 = 0; //Not applicable

    fn read<R: Read>(reader: R, _: ()) -> io::Result<Self> {
        Self::read(reader)
    }

    fn write<W: Write>(&self, writer: W) -> io::Result<()> {
        self.write(writer)
    }
}

impl ReadableWriteable<()> for OrchardFullViewingKey {
    const VERSION: u8 = 0; //Not applicable

    fn read<R: Read>(reader: R, _: ()) -> io::Result<Self> {
        Self::read(reader)
    }

    fn write<W: Write>(&self, writer: W) -> io::Result<()> {
        self.write(writer)
    }
}

impl ReadableWriteable<(SaplingExtendedFullViewingKey, SaplingDiversifier)> for SaplingNote {
    const VERSION: u8 = 1;

    fn read<R: Read>(
        mut reader: R,
        (extfvk, diversifier): (SaplingExtendedFullViewingKey, SaplingDiversifier),
    ) -> io::Result<Self> {
        let _version = Self::get_version(&mut reader)?;
        let value = reader.read_u64::<LittleEndian>()?;
        let rseed = super::data::read_sapling_rseed(&mut reader)?;

        let maybe_note = extfvk
            .fvk
            .vk
            .to_payment_address(diversifier)
            .unwrap()
            .create_note(value, rseed);

        match maybe_note {
            Some(n) => Ok(n),
            None => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Couldn't create the note for the address",
            )),
        }
    }

    fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_u8(Self::VERSION)?;
        writer.write_u64::<LittleEndian>(self.value)?;
        super::data::write_sapling_rseed(&mut writer, &self.rseed)?;
        Ok(())
    }
}

impl ReadableWriteable<(OrchardFullViewingKey, OrchardDiversifier)> for OrchardNote {
    const VERSION: u8 = 1;

    fn read<R: Read>(
        mut reader: R,
        (fvk, diversifier): (OrchardFullViewingKey, OrchardDiversifier),
    ) -> io::Result<Self> {
        let _version = Self::get_version(&mut reader)?;
        let value = reader.read_u64::<LittleEndian>()?;
        let mut nullifier_bytes = [0; 32];
        reader.read_exact(&mut nullifier_bytes)?;
        let nullifier = Option::from(OrchardNullifier::from_bytes(&nullifier_bytes))
            .ok_or(io::Error::new(io::ErrorKind::InvalidInput, "Bad Nullifier"))?;

        let mut random_seed_bytes = [0; 32];
        reader.read_exact(&mut random_seed_bytes)?;
        let random_seed = Option::from(orchard::note::RandomSeed::from_bytes(
            random_seed_bytes,
            &nullifier,
        ))
        .ok_or(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Nullifier not for note",
        ))?;

        Option::from(OrchardNote::from_parts(
            fvk.address(diversifier, orchard::keys::Scope::External),
            orchard::value::NoteValue::from_raw(value),
            nullifier,
            random_seed,
        ))
        .ok_or(io::Error::new(io::ErrorKind::InvalidInput, "Invalid note"))
    }

    fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_u8(Self::VERSION)?;
        writer.write_u64::<LittleEndian>(self.value().inner())?;
        writer.write_all(&self.rho().to_bytes())?;
        writer.write_all(self.random_seed().as_bytes())?;
        Ok(())
    }
}

impl<T> ReadableWriteable<()> for T
where
    T: ReceivedNoteAndMetadata,
{
    const VERSION: u8 = 1;

    fn read<R: Read>(mut reader: R, _: ()) -> io::Result<Self> {
        let _version = Self::get_version(&mut reader)?;

        let fvk = <T::Fvk as ReadableWriteable<()>>::read(&mut reader, ())?;

        let mut diversifier_bytes = [0u8; 11];
        reader.read_exact(&mut diversifier_bytes)?;
        let diversifier = T::Diversifier::from_bytes(diversifier_bytes);

        let note =
            <T::Note as ReadableWriteable<_>>::read(&mut reader, (fvk.clone(), diversifier))?;

        let witnesses_vec = Vector::read(&mut reader, |r| IncrementalWitness::<T::Node>::read(r))?;
        let top_height = reader.read_u64::<LittleEndian>()?;
        let witnesses = WitnessCache::new(witnesses_vec, top_height);

        let mut nullifier = [0u8; 32];
        reader.read_exact(&mut nullifier)?;
        let nullifier = T::Nullifier::from_bytes(nullifier);

        // Note that this is only the spent field, we ignore the unconfirmed_spent field.
        // The reason is that unconfirmed spents are only in memory, and we need to get the actual value of spent
        // from the blockchain anyway.
        let spent = Optional::read(&mut reader, |r| {
            let mut transaction_id_bytes = [0u8; 32];
            r.read_exact(&mut transaction_id_bytes)?;
            let height = r.read_u32::<LittleEndian>()?;
            Ok((TxId::from_bytes(transaction_id_bytes), height))
        })?;

        let unconfirmed_spent = {
            Optional::read(&mut reader, |r| {
                let mut transaction_bytes = [0u8; 32];
                r.read_exact(&mut transaction_bytes)?;

                let height = r.read_u32::<LittleEndian>()?;
                Ok((TxId::from_bytes(transaction_bytes), height))
            })?
        };

        let memo = Optional::read(&mut reader, |r| {
            let mut memo_bytes = [0u8; 512];
            r.read_exact(&mut memo_bytes)?;

            // Attempt to read memo, first as text, else as arbitrary 512 bytes
            match MemoBytes::from_bytes(&memo_bytes) {
                Ok(mb) => match Memo::try_from(mb.clone()) {
                    Ok(m) => Ok(m),
                    Err(_) => Ok(Memo::Future(mb)),
                },
                Err(e) => Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Couldn't create memo: {}", e),
                )),
            }
        })?;

        let is_change: bool = reader.read_u8()? > 0;

        let have_spending_key = reader.read_u8()? > 0;

        Ok(T::from_parts(
            fvk,
            diversifier,
            note,
            witnesses,
            nullifier,
            spent,
            unconfirmed_spent,
            memo,
            is_change,
            have_spending_key,
        ))
    }

    fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        // Write a version number first, so we can later upgrade this if needed.
        writer.write_u8(Self::VERSION)?;

        self.fvk().write(&mut writer)?;

        writer.write_all(&self.diversifier().to_bytes())?;

        self.note().write(&mut writer)?;
        Vector::write(&mut writer, &self.witnesses().witnesses, |wr, wi| {
            wi.write(wr)
        })?;
        writer.write_u64::<LittleEndian>(self.witnesses().top_height)?;

        writer.write_all(&self.nullifier().to_bytes())?;

        Optional::write(
            &mut writer,
            self.spent().as_ref(),
            |w, (transaction_id, height)| {
                w.write_all(transaction_id.as_ref())?;
                w.write_u32::<LittleEndian>(*height)
            },
        )?;

        Optional::write(
            &mut writer,
            self.unconfirmed_spent().as_ref(),
            |w, (transaction_id, height)| {
                w.write_all(transaction_id.as_ref())?;
                w.write_u32::<LittleEndian>(*height)
            },
        )?;

        Optional::write(&mut writer, self.memo().as_ref(), |w, m| {
            w.write_all(m.encode().as_array())
        })?;

        writer.write_u8(if self.is_change() { 1 } else { 0 })?;

        writer.write_u8(if self.have_spending_key() { 1 } else { 0 })?;

        //TODO: Investigate this comment. It may be the solution to the potential bug
        //we are looking at, and it seems to no lopnger be true.

        // Note that we don't write the unconfirmed_spent field, because if the wallet is restarted,
        // we don't want to be beholden to any expired transactions

        Ok(())
    }
}

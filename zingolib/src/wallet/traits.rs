//! Provides unifying interfaces for transaction management across Sapling and Orchard
use std::{
    io::{self, Read, Write},
    sync::Arc,
};

use super::{
    data::{
        merkle::SqliteShardStore, PoolNullifier, ReceivedOrchardNoteAndMetadata,
        ReceivedSaplingNoteAndMetadata, SpendableOrchardNote, SpendableSaplingNote,
        TransactionMetadata, WitnessCache, COMMITMENT_TREE_DEPTH, MAX_SHARD_DEPTH,
    },
    keys::unified::WalletCapability,
    transactions::TransactionMetadataSet,
    Pool,
};
use crate::compact_formats::{
    slice_to_array, CompactOrchardAction, CompactSaplingOutput, CompactTx, TreeState,
};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use incrementalmerkletree::{Hashable, Level, Position};
use nonempty::NonEmpty;
use orchard::{
    note_encryption::OrchardDomain,
    primitives::redpallas::{Signature, SpendAuth},
    tree::MerkleHashOrchard,
    Action,
};
use shardtree::{memory::MemoryShardStore, ShardTree};
use subtle::CtOption;
use tokio::sync::Mutex;
use zcash_address::unified::{self, Receiver};
use zcash_client_backend::{address::UnifiedAddress, encoding::encode_payment_address};
use zcash_encoding::{Optional, Vector};
use zcash_note_encryption::{
    BatchDomain, Domain, ShieldedOutput, COMPACT_NOTE_SIZE, ENC_CIPHERTEXT_SIZE,
};
use zcash_primitives::{
    consensus::{BlockHeight, NetworkUpgrade, Parameters},
    memo::{Memo, MemoBytes},
    merkle_tree::{read_incremental_witness, HashSer},
    sapling::note_encryption::SaplingDomain,
    transaction::{
        components::{self, sapling::GrothProofBytes, Amount, OutputDescription, SpendDescription},
        Transaction, TxId,
    },
    zip32,
};
use zingoconfig::ChainType;

/// This provides a uniform `.to_bytes` to types that might require it in a generic context.
pub trait ToBytes<const N: usize> {
    fn to_bytes(&self) -> [u8; N];
}

impl ToBytes<32> for zcash_primitives::sapling::Nullifier {
    fn to_bytes(&self) -> [u8; 32] {
        self.0
    }
}

impl ToBytes<32> for orchard::note::Nullifier {
    fn to_bytes(&self) -> [u8; 32] {
        orchard::note::Nullifier::to_bytes(*self)
    }
}

impl ToBytes<11> for zcash_primitives::sapling::Diversifier {
    fn to_bytes(&self) -> [u8; 11] {
        self.0
    }
}

impl ToBytes<11> for orchard::keys::Diversifier {
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
        *self.out_ciphertext()
    }

    fn value_commitment(&self) -> <SaplingDomain<ChainType> as Domain>::ValueCommitment {
        self.cv().clone()
    }
}

/// Provides a standard `from_bytes` interface to be used generically
pub trait FromBytes<const N: usize> {
    fn from_bytes(bytes: [u8; N]) -> Self;
}

impl FromBytes<32> for zcash_primitives::sapling::Nullifier {
    fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl FromBytes<32> for orchard::note::Nullifier {
    fn from_bytes(bytes: [u8; 32]) -> Self {
        Option::from(orchard::note::Nullifier::from_bytes(&bytes))
            .unwrap_or_else(|| panic!("Invalid nullifier {:?}", bytes))
    }
}

impl FromBytes<11> for zcash_primitives::sapling::Diversifier {
    fn from_bytes(bytes: [u8; 11]) -> Self {
        zcash_primitives::sapling::Diversifier(bytes)
    }
}

impl FromBytes<11> for orchard::keys::Diversifier {
    fn from_bytes(bytes: [u8; 11]) -> Self {
        orchard::keys::Diversifier::from_bytes(bytes)
    }
}

pub trait FromCommitment
where
    Self: Sized,
{
    fn from_commitment(from: &[u8; 32]) -> CtOption<Self>;
}

impl FromCommitment for zcash_primitives::sapling::Node {
    fn from_commitment(from: &[u8; 32]) -> CtOption<Self> {
        let maybe_node =
            <zcash_primitives::sapling::Node as zcash_primitives::merkle_tree::HashSer>::read(
                from.as_slice(),
            );
        match maybe_node {
            Ok(node) => CtOption::new(node, subtle::Choice::from(1)),
            Err(_) => CtOption::new(Self::empty_root(Level::from(0)), subtle::Choice::from(0)),
        }
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

impl<Auth: zcash_primitives::transaction::components::sapling::Authorization> Spend
    for SpendDescription<Auth>
{
    type Nullifier = zcash_primitives::sapling::Nullifier;
    fn nullifier(&self) -> &Self::Nullifier {
        self.nullifier()
    }
}

impl<Auth> Spend for Action<Auth> {
    type Nullifier = orchard::note::Nullifier;
    fn nullifier(&self) -> &Self::Nullifier {
        self.nullifier()
    }
}

impl From<orchard::note::Nullifier> for PoolNullifier {
    fn from(n: orchard::note::Nullifier) -> Self {
        PoolNullifier::Orchard(n)
    }
}

impl From<zcash_primitives::sapling::Nullifier> for PoolNullifier {
    fn from(n: zcash_primitives::sapling::Nullifier) -> Self {
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

impl Recipient for orchard::Address {
    type Diversifier = orchard::keys::Diversifier;

    fn diversifier(&self) -> Self::Diversifier {
        orchard::Address::diversifier(self)
    }

    fn b32encode_for_network(&self, chain: &ChainType) -> String {
        unified::Encoding::encode(
            &<unified::Address as unified::Encoding>::try_from_items(vec![Receiver::Orchard(
                self.to_raw_address_bytes(),
            )])
            .expect("Could not create UA from orchard address"),
            &chain.address_network().unwrap(),
        )
    }
}

impl Recipient for zcash_primitives::sapling::PaymentAddress {
    type Diversifier = zcash_primitives::sapling::Diversifier;

    fn diversifier(&self) -> Self::Diversifier {
        *zcash_primitives::sapling::PaymentAddress::diversifier(self)
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
        slice_to_array(&self.cmu)
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
        slice_to_array(&self.cmx)
    }

    fn domain(&self, _parameters: ChainType, _heightt: BlockHeight) -> OrchardDomain {
        OrchardDomain::for_nullifier(
            orchard::note::Nullifier::from_bytes(slice_to_array(&self.nullifier)).unwrap(),
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

impl Bundle<SaplingDomain<ChainType>>
    for components::sapling::Bundle<components::sapling::Authorized>
{
    type Spend = SpendDescription<components::sapling::Authorized>;
    type Output = OutputDescription<GrothProofBytes>;
    type Spends<'a> = &'a [Self::Spend];
    type Outputs<'a> = &'a [Self::Output];
    fn from_transaction(transaction: &Transaction) -> Option<&Self> {
        transaction.sapling_bundle()
    }

    fn output_elements(&self) -> Self::Outputs<'_> {
        self.shielded_outputs()
    }

    fn spend_elements(&self) -> Self::Spends<'_> {
        self.shielded_spends()
    }
}

impl Bundle<OrchardDomain> for orchard::bundle::Bundle<orchard::bundle::Authorized, Amount> {
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

impl Nullifier for zcash_primitives::sapling::Nullifier {
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

impl Nullifier for orchard::note::Nullifier {
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
    type Diversifier: Copy + FromBytes<11> + ToBytes<11>;

    type Note: PartialEq
        + for<'a> ReadableWriteable<(Self::Diversifier, &'a WalletCapability)>
        + Clone;
    type Node: Hashable + HashSer + FromCommitment + Send + Clone + PartialEq + Eq;
    type Nullifier: Nullifier;

    fn is_pending(&self) -> bool {
        self.nullifier() == Self::Nullifier::from_bytes([0; 32])
    }
    fn diversifier(&self) -> &Self::Diversifier;
    #[allow(clippy::too_many_arguments)]
    fn from_parts(
        diversifier: Self::Diversifier,
        note: Self::Note,
        witness_position: Position,
        nullifier: Option<Self::Nullifier>,
        spent: Option<(TxId, u32)>,
        unconfirmed_spent: Option<(TxId, u32)>,
        memo: Option<Memo>,
        is_change: bool,
        have_spending_key: bool,
        output_index: usize,
    ) -> Self;
    fn get_deprecated_serialized_view_key_buffer() -> Vec<u8>;
    fn have_spending_key(&self) -> bool;
    fn is_change(&self) -> bool;
    fn is_change_mut(&mut self) -> &mut bool;
    fn is_spent(&self) -> bool {
        Self::spent(self).is_some()
    }
    fn output_index(&self) -> &usize;
    fn output_index_mut(&mut self) -> &mut usize;
    fn memo(&self) -> &Option<Memo>;
    fn memo_mut(&mut self) -> &mut Option<Memo>;
    fn note(&self) -> &Self::Note;
    fn nullifier(&self) -> Self::Nullifier;
    fn nullifier_mut(&mut self) -> &mut Self::Nullifier;
    fn pool() -> Pool;
    fn spent(&self) -> &Option<(TxId, u32)>;
    fn spent_mut(&mut self) -> &mut Option<(TxId, u32)>;
    fn transaction_metadata_notes(wallet_transaction: &TransactionMetadata) -> &Vec<Self>;
    fn transaction_metadata_notes_mut(
        wallet_transaction: &mut TransactionMetadata,
    ) -> &mut Vec<Self>;
    fn unconfirmed_spent(&self) -> &Option<(TxId, u32)>;
    fn unconfirmed_spent_mut(&mut self) -> &mut Option<(TxId, u32)>;
    ///Convenience function
    fn value(&self) -> u64 {
        Self::value_from_note(self.note())
    }
    fn value_from_note(note: &Self::Note) -> u64;
    fn witnessed_position(&self) -> &Position;
    fn witnessed_position_mut(&mut self) -> &mut Position;
}

impl ReceivedNoteAndMetadata for ReceivedSaplingNoteAndMetadata {
    type Diversifier = zcash_primitives::sapling::Diversifier;
    type Note = zcash_primitives::sapling::Note;
    type Node = zcash_primitives::sapling::Node;
    type Nullifier = zcash_primitives::sapling::Nullifier;

    fn diversifier(&self) -> &Self::Diversifier {
        &self.diversifier
    }

    fn nullifier_mut(&mut self) -> &mut Self::Nullifier {
        &mut self.nullifier
    }

    fn from_parts(
        diversifier: zcash_primitives::sapling::Diversifier,
        note: zcash_primitives::sapling::Note,
        witnessed_position: Position,
        nullifier: Option<zcash_primitives::sapling::Nullifier>,
        spent: Option<(TxId, u32)>,
        unconfirmed_spent: Option<(TxId, u32)>,
        memo: Option<Memo>,
        is_change: bool,
        have_spending_key: bool,
        output_index: usize,
    ) -> Self {
        Self {
            diversifier,
            note,
            witnessed_position,
            nullifier: nullifier
                .unwrap_or(zcash_primitives::sapling::Nullifier::from_bytes([0; 32])),
            spent,
            unconfirmed_spent,
            memo,
            is_change,
            have_spending_key,
            output_index,
        }
    }

    fn get_deprecated_serialized_view_key_buffer() -> Vec<u8> {
        vec![0u8; 169]
    }

    fn have_spending_key(&self) -> bool {
        self.have_spending_key
    }

    fn is_change(&self) -> bool {
        self.is_change
    }

    fn is_change_mut(&mut self) -> &mut bool {
        &mut self.is_change
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

    fn nullifier(&self) -> Self::Nullifier {
        self.nullifier
    }

    fn pool() -> Pool {
        Pool::Sapling
    }

    fn spent(&self) -> &Option<(TxId, u32)> {
        &self.spent
    }

    fn spent_mut(&mut self) -> &mut Option<(TxId, u32)> {
        &mut self.spent
    }

    fn transaction_metadata_notes(wallet_transaction: &TransactionMetadata) -> &Vec<Self> {
        &wallet_transaction.sapling_notes
    }

    fn transaction_metadata_notes_mut(
        wallet_transaction: &mut TransactionMetadata,
    ) -> &mut Vec<Self> {
        &mut wallet_transaction.sapling_notes
    }

    fn unconfirmed_spent(&self) -> &Option<(TxId, u32)> {
        &self.unconfirmed_spent
    }

    fn unconfirmed_spent_mut(&mut self) -> &mut Option<(TxId, u32)> {
        &mut self.unconfirmed_spent
    }

    fn value_from_note(note: &Self::Note) -> u64 {
        note.value().inner()
    }

    fn witnessed_position(&self) -> &Position {
        &self.witnessed_position
    }

    fn witnessed_position_mut(&mut self) -> &mut Position {
        &mut self.witnessed_position
    }

    fn output_index(&self) -> &usize {
        &self.output_index
    }

    fn output_index_mut(&mut self) -> &mut usize {
        &mut self.output_index
    }
}

impl ReceivedNoteAndMetadata for ReceivedOrchardNoteAndMetadata {
    type Diversifier = orchard::keys::Diversifier;
    type Note = orchard::note::Note;
    type Node = MerkleHashOrchard;
    type Nullifier = orchard::note::Nullifier;

    fn diversifier(&self) -> &Self::Diversifier {
        &self.diversifier
    }

    fn nullifier_mut(&mut self) -> &mut Self::Nullifier {
        &mut self.nullifier
    }

    fn from_parts(
        diversifier: Self::Diversifier,
        note: Self::Note,
        witnessed_position: Position,
        nullifier: Option<Self::Nullifier>,
        spent: Option<(TxId, u32)>,
        unconfirmed_spent: Option<(TxId, u32)>,
        memo: Option<Memo>,
        is_change: bool,
        have_spending_key: bool,
        output_index: usize,
    ) -> Self {
        Self {
            diversifier,
            note,
            witnessed_position,
            nullifier: nullifier.unwrap_or(<Self::Nullifier as FromBytes<32>>::from_bytes([0; 32])),
            spent,
            unconfirmed_spent,
            memo,
            is_change,
            have_spending_key,
            output_index,
        }
    }

    fn get_deprecated_serialized_view_key_buffer() -> Vec<u8> {
        vec![0u8; 96]
    }

    fn have_spending_key(&self) -> bool {
        self.have_spending_key
    }
    fn is_change(&self) -> bool {
        self.is_change
    }

    fn is_change_mut(&mut self) -> &mut bool {
        &mut self.is_change
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

    fn nullifier(&self) -> Self::Nullifier {
        self.nullifier
    }

    fn pool() -> Pool {
        Pool::Orchard
    }

    fn spent(&self) -> &Option<(TxId, u32)> {
        &self.spent
    }

    fn spent_mut(&mut self) -> &mut Option<(TxId, u32)> {
        &mut self.spent
    }

    fn transaction_metadata_notes(wallet_transaction: &TransactionMetadata) -> &Vec<Self> {
        &wallet_transaction.orchard_notes
    }

    fn transaction_metadata_notes_mut(
        wallet_transaction: &mut TransactionMetadata,
    ) -> &mut Vec<Self> {
        &mut wallet_transaction.orchard_notes
    }

    fn unconfirmed_spent(&self) -> &Option<(TxId, u32)> {
        &self.unconfirmed_spent
    }

    fn unconfirmed_spent_mut(&mut self) -> &mut Option<(TxId, u32)> {
        &mut self.unconfirmed_spent
    }

    fn value_from_note(note: &Self::Note) -> u64 {
        note.value().inner()
    }

    fn witnessed_position(&self) -> &Position {
        &self.witnessed_position
    }
    fn witnessed_position_mut(&mut self) -> &mut Position {
        &mut self.witnessed_position
    }
    fn output_index(&self) -> &usize {
        &self.output_index
    }

    fn output_index_mut(&mut self) -> &mut usize {
        &mut self.output_index
    }
}

pub trait DomainWalletExt: Domain + BatchDomain
where
    Self: Sized,
    Self::Note: PartialEq + Clone,
    Self::Recipient: Recipient,
{
    const NU: NetworkUpgrade;

    type Fvk: Clone + Send + Diversifiable<Note = Self::WalletNote, Address = Self::Recipient>;

    type SpendingKey: for<'a> TryFrom<&'a WalletCapability> + Clone;
    type CompactOutput: CompactOutput<Self>;
    type WalletNote: ReceivedNoteAndMetadata<
        Note = <Self as Domain>::Note,
        Diversifier = <<Self as Domain>::Recipient as Recipient>::Diversifier,
        Nullifier = <<<Self as DomainWalletExt>::Bundle as Bundle<Self>>::Spend as Spend>::Nullifier,
    > + std::fmt::Debug;
    type SpendableNoteAT: SpendableNote<Self>;

    type Bundle: Bundle<Self>;

    fn sum_pool_change(transaction_md: &TransactionMetadata) -> u64 {
        Self::to_notes_vec(transaction_md)
            .iter()
            .filter(|nd| nd.is_change())
            .map(|nd| nd.value())
            .sum()
    }
    fn get_shardtree(
        tmds: &TransactionMetadataSet,
    ) -> Arc<
        Mutex<
            ShardTree<
                MemoryShardStore<<Self::WalletNote as ReceivedNoteAndMetadata>::Node, BlockHeight>,
                COMMITMENT_TREE_DEPTH,
                MAX_SHARD_DEPTH,
            >,
        >,
    >;
    fn get_nullifier_from_note_fvk_and_witness_position(
        note: &Self::Note,
        fvk: &Self::Fvk,
        position: u64,
    ) -> <Self::WalletNote as ReceivedNoteAndMetadata>::Nullifier;
    fn get_tree(tree_state: &TreeState) -> &String;
    fn to_notes_vec(_: &TransactionMetadata) -> &Vec<Self::WalletNote>;
    fn to_notes_vec_mut(_: &mut TransactionMetadata) -> &mut Vec<Self::WalletNote>;
    fn ua_from_contained_receiver<'a>(
        unified_spend_auth: &'a WalletCapability,
        receiver: &Self::Recipient,
    ) -> Option<&'a UnifiedAddress>;
    fn wc_to_fvk(wc: &WalletCapability) -> Result<Self::Fvk, String>;
    fn wc_to_ivk(wc: &WalletCapability) -> Result<Self::IncomingViewingKey, String>;
    fn wc_to_ovk(wc: &WalletCapability) -> Result<Self::OutgoingViewingKey, String>;
    fn wc_to_sk(wc: &WalletCapability) -> Result<Self::SpendingKey, String>;
}

impl DomainWalletExt for SaplingDomain<ChainType> {
    const NU: NetworkUpgrade = NetworkUpgrade::Sapling;

    type Fvk = zip32::sapling::DiversifiableFullViewingKey;

    type SpendingKey = zip32::sapling::ExtendedSpendingKey;

    type CompactOutput = CompactSaplingOutput;

    type WalletNote = ReceivedSaplingNoteAndMetadata;

    type SpendableNoteAT = SpendableSaplingNote;

    type Bundle = components::sapling::Bundle<components::sapling::Authorized>;

    fn get_shardtree(
        tmds: &TransactionMetadataSet,
    ) -> Arc<
        Mutex<
            ShardTree<
                MemoryShardStore<<Self::WalletNote as ReceivedNoteAndMetadata>::Node, BlockHeight>,
                COMMITMENT_TREE_DEPTH,
                MAX_SHARD_DEPTH,
            >,
        >,
    > {
        tmds.witness_trees.witness_tree_sapling.clone()
    }
    fn get_nullifier_from_note_fvk_and_witness_position(
        note: &Self::Note,
        fvk: &Self::Fvk,
        position: u64,
    ) -> <<Self as DomainWalletExt>::WalletNote as ReceivedNoteAndMetadata>::Nullifier {
        note.nf(&fvk.fvk().vk.nk, position)
    }

    fn get_tree(tree_state: &TreeState) -> &String {
        &tree_state.sapling_tree
    }

    fn to_notes_vec(transaction_md: &TransactionMetadata) -> &Vec<Self::WalletNote> {
        &transaction_md.sapling_notes
    }

    fn to_notes_vec_mut(transaction: &mut TransactionMetadata) -> &mut Vec<Self::WalletNote> {
        &mut transaction.sapling_notes
    }
    fn ua_from_contained_receiver<'a>(
        unified_spend_auth: &'a WalletCapability,
        receiver: &Self::Recipient,
    ) -> Option<&'a UnifiedAddress> {
        unified_spend_auth
            .addresses()
            .iter()
            .find(|ua| ua.sapling() == Some(receiver))
    }
    fn wc_to_fvk(wc: &WalletCapability) -> Result<Self::Fvk, String> {
        Self::Fvk::try_from(wc)
    }
    fn wc_to_ivk(wc: &WalletCapability) -> Result<Self::IncomingViewingKey, String> {
        Self::IncomingViewingKey::try_from(wc)
    }
    fn wc_to_ovk(wc: &WalletCapability) -> Result<Self::OutgoingViewingKey, String> {
        Self::OutgoingViewingKey::try_from(wc)
    }

    fn wc_to_sk(wc: &WalletCapability) -> Result<Self::SpendingKey, String> {
        Self::SpendingKey::try_from(wc)
    }
}

impl DomainWalletExt for OrchardDomain {
    const NU: NetworkUpgrade = NetworkUpgrade::Nu5;

    type Fvk = orchard::keys::FullViewingKey;

    type SpendingKey = orchard::keys::SpendingKey;

    type CompactOutput = CompactOrchardAction;

    type WalletNote = ReceivedOrchardNoteAndMetadata;

    type SpendableNoteAT = SpendableOrchardNote;

    type Bundle = orchard::bundle::Bundle<orchard::bundle::Authorized, Amount>;

    fn get_shardtree(
        tmds: &TransactionMetadataSet,
    ) -> Arc<
        Mutex<
            ShardTree<
                MemoryShardStore<<Self::WalletNote as ReceivedNoteAndMetadata>::Node, BlockHeight>,
                COMMITMENT_TREE_DEPTH,
                MAX_SHARD_DEPTH,
            >,
        >,
    > {
        tmds.witness_trees.witness_tree_orchard.clone()
    }
    fn get_nullifier_from_note_fvk_and_witness_position(
        note: &Self::Note,
        fvk: &Self::Fvk,
        _position: u64,
    ) -> <<Self as DomainWalletExt>::WalletNote as ReceivedNoteAndMetadata>::Nullifier {
        note.nullifier(fvk)
    }

    fn get_tree(tree_state: &TreeState) -> &String {
        &tree_state.orchard_tree
    }

    fn to_notes_vec(transaction_md: &TransactionMetadata) -> &Vec<Self::WalletNote> {
        &transaction_md.orchard_notes
    }

    fn to_notes_vec_mut(transaction: &mut TransactionMetadata) -> &mut Vec<Self::WalletNote> {
        &mut transaction.orchard_notes
    }
    fn ua_from_contained_receiver<'a>(
        unified_spend_capability: &'a WalletCapability,
        receiver: &Self::Recipient,
    ) -> Option<&'a UnifiedAddress> {
        unified_spend_capability
            .addresses()
            .iter()
            .find(|unified_address| unified_address.orchard() == Some(receiver))
    }
    fn wc_to_fvk(wc: &WalletCapability) -> Result<Self::Fvk, String> {
        Self::Fvk::try_from(wc)
    }
    fn wc_to_ivk(wc: &WalletCapability) -> Result<Self::IncomingViewingKey, String> {
        Self::IncomingViewingKey::try_from(wc)
    }
    fn wc_to_ovk(wc: &WalletCapability) -> Result<Self::OutgoingViewingKey, String> {
        Self::OutgoingViewingKey::try_from(wc)
    }

    fn wc_to_sk(wc: &WalletCapability) -> Result<Self::SpendingKey, String> {
        Self::SpendingKey::try_from(wc)
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

impl Diversifiable for zip32::sapling::DiversifiableFullViewingKey {
    type Note = ReceivedSaplingNoteAndMetadata;

    type Address = zcash_primitives::sapling::PaymentAddress;

    fn diversified_address(
        &self,
        div: <<zip32::sapling::DiversifiableFullViewingKey as Diversifiable>::Note as ReceivedNoteAndMetadata>::Diversifier,
    ) -> Option<Self::Address> {
        self.fvk().vk.to_payment_address(div)
    }
}

impl Diversifiable for orchard::keys::FullViewingKey {
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
        spend_key: Option<&D::SpendingKey>,
    ) -> Option<Self> {
        // Include only notes that haven't been spent, or haven't been included in an unconfirmed spend yet.
        if note_and_metadata.spent().is_none()
            && note_and_metadata.unconfirmed_spent().is_none()
            && spend_key.is_some()
        //TODO: Account for lack of this line
        // && note_and_metadata.witnessed_position().len() >= (anchor_offset + 1)
        {
            Some(Self::from_parts_unchecked(
                transaction_id,
                note_and_metadata.nullifier(),
                *note_and_metadata.diversifier(),
                note_and_metadata.note().clone(),
                *note_and_metadata.witnessed_position(),
                spend_key,
            ))
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
        witnessed_position: Position,
        sk: Option<&D::SpendingKey>,
    ) -> Self;
    fn transaction_id(&self) -> TxId;
    fn nullifier(&self) -> <D::WalletNote as ReceivedNoteAndMetadata>::Nullifier;
    fn diversifier(&self) -> <D::WalletNote as ReceivedNoteAndMetadata>::Diversifier;
    fn note(&self) -> &D::Note;
    fn witnessed_position(&self) -> &Position;
    fn spend_key(&self) -> Option<&D::SpendingKey>;
}

impl SpendableNote<SaplingDomain<ChainType>> for SpendableSaplingNote {
    fn from_parts_unchecked(
        transaction_id: TxId,
        nullifier: zcash_primitives::sapling::Nullifier,
        diversifier: zcash_primitives::sapling::Diversifier,
        note: zcash_primitives::sapling::Note,
        witnessed_position: Position,
        extsk: Option<&zip32::sapling::ExtendedSpendingKey>,
    ) -> Self {
        SpendableSaplingNote {
            transaction_id,
            nullifier,
            diversifier,
            note,
            witnessed_position,
            extsk: extsk.cloned(),
        }
    }

    fn transaction_id(&self) -> TxId {
        self.transaction_id
    }

    fn nullifier(&self) -> zcash_primitives::sapling::Nullifier {
        self.nullifier
    }

    fn diversifier(&self) -> zcash_primitives::sapling::Diversifier {
        self.diversifier
    }

    fn note(&self) -> &zcash_primitives::sapling::Note {
        &self.note
    }

    fn witnessed_position(&self) -> &Position {
        &self.witnessed_position
    }

    fn spend_key(&self) -> Option<&zip32::sapling::ExtendedSpendingKey> {
        self.extsk.as_ref()
    }
}

impl SpendableNote<OrchardDomain> for SpendableOrchardNote {
    fn from_parts_unchecked(
        transaction_id: TxId,
        nullifier: orchard::note::Nullifier,
        diversifier: orchard::keys::Diversifier,
        note: orchard::note::Note,
        witnessed_position: Position,
        sk: Option<&orchard::keys::SpendingKey>,
    ) -> Self {
        SpendableOrchardNote {
            transaction_id,
            nullifier,
            diversifier,
            note,
            witnessed_position,
            spend_key: sk.cloned(),
        }
    }
    fn transaction_id(&self) -> TxId {
        self.transaction_id
    }

    fn nullifier(&self) -> orchard::note::Nullifier {
        self.nullifier
    }

    fn diversifier(&self) -> orchard::keys::Diversifier {
        self.diversifier
    }

    fn note(&self) -> &orchard::Note {
        &self.note
    }

    fn witnessed_position(&self) -> &Position {
        &self.witnessed_position
    }

    fn spend_key(&self) -> Option<&orchard::keys::SpendingKey> {
        self.spend_key.as_ref()
    }
}

pub trait ReadableWriteable<Input>: Sized {
    const VERSION: u8;

    fn read<R: Read>(reader: R, input: Input) -> io::Result<Self>;
    fn write<W: Write>(&self, writer: W) -> io::Result<()>;
    fn get_version<R: Read>(mut reader: R) -> io::Result<u8> {
        let external_version = reader.read_u8()?;
        log::info!("wallet_capability external_version: {external_version}");
        log::info!("Self::VERSION: {}", Self::VERSION);
        if external_version > Self::VERSION {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Wallet file version \"{}\" is from future version of zingo",
                    external_version,
                ),
            ))
        } else {
            Ok(external_version)
        }
    }
}

impl ReadableWriteable<()> for orchard::keys::SpendingKey {
    const VERSION: u8 = 0; //Not applicable

    fn read<R: Read>(mut reader: R, _: ()) -> io::Result<Self> {
        let mut data = [0u8; 32];
        reader.read_exact(&mut data)?;

        Option::from(Self::from_bytes(data)).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unable to deserialize a valid Orchard SpendingKey from bytes".to_owned(),
            )
        })
    }

    fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_all(self.to_bytes())
    }
}

impl ReadableWriteable<()> for zip32::sapling::ExtendedSpendingKey {
    const VERSION: u8 = 0; //Not applicable

    fn read<R: Read>(reader: R, _: ()) -> io::Result<Self> {
        Self::read(reader)
    }

    fn write<W: Write>(&self, writer: W) -> io::Result<()> {
        self.write(writer)
    }
}

impl ReadableWriteable<()> for zip32::sapling::DiversifiableFullViewingKey {
    const VERSION: u8 = 0; //Not applicable

    fn read<R: Read>(mut reader: R, _: ()) -> io::Result<Self> {
        let mut fvk_bytes = [0u8; 128];
        reader.read_exact(&mut fvk_bytes)?;
        tracing::info!(
            "zip32::sapling::DiversifiableFullViewingKey fvk_bytes: {:?}",
            fvk_bytes
        );
        zip32::sapling::DiversifiableFullViewingKey::from_bytes(&fvk_bytes).ok_or(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Couldn't read a Sapling Diversifiable Full Viewing Key",
        ))
    }

    fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_all(&self.to_bytes())
    }
}

impl ReadableWriteable<()> for orchard::keys::FullViewingKey {
    const VERSION: u8 = 0; //Not applicable

    fn read<R: Read>(reader: R, _: ()) -> io::Result<Self> {
        Self::read(reader)
    }

    fn write<W: Write>(&self, writer: W) -> io::Result<()> {
        self.write(writer)
    }
}

impl ReadableWriteable<(zcash_primitives::sapling::Diversifier, &WalletCapability)>
    for zcash_primitives::sapling::Note
{
    const VERSION: u8 = 1;

    fn read<R: Read>(
        mut reader: R,
        (diversifier, wallet_capability): (
            zcash_primitives::sapling::Diversifier,
            &WalletCapability,
        ),
    ) -> io::Result<Self> {
        let _version = Self::get_version(&mut reader)?;
        let value = reader.read_u64::<LittleEndian>()?;
        let rseed = super::data::read_sapling_rseed(&mut reader)?;

        Ok(
            <SaplingDomain<zingoconfig::ChainType> as DomainWalletExt>::wc_to_fvk(
                wallet_capability,
            )
            .expect("to get an fvk from a wc")
            .fvk()
            .vk
            .to_payment_address(diversifier)
            .unwrap()
            .create_note(value, rseed),
        )
    }

    fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_u8(Self::VERSION)?;
        writer.write_u64::<LittleEndian>(self.value().inner())?;
        super::data::write_sapling_rseed(&mut writer, self.rseed())?;
        Ok(())
    }
}

impl ReadableWriteable<(orchard::keys::Diversifier, &WalletCapability)> for orchard::note::Note {
    const VERSION: u8 = 1;

    fn read<R: Read>(
        mut reader: R,
        (diversifier, wallet_capability): (orchard::keys::Diversifier, &WalletCapability),
    ) -> io::Result<Self> {
        let _version = Self::get_version(&mut reader)?;
        let value = reader.read_u64::<LittleEndian>()?;
        let mut nullifier_bytes = [0; 32];
        reader.read_exact(&mut nullifier_bytes)?;
        let nullifier = Option::from(orchard::note::Nullifier::from_bytes(&nullifier_bytes))
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

        let fvk = <OrchardDomain as DomainWalletExt>::wc_to_fvk(wallet_capability)
            .expect("to get an fvk from a wc");
        Option::from(orchard::note::Note::from_parts(
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
        writer.write_all(self.rseed().as_bytes())?;
        Ok(())
    }
}

impl<T> ReadableWriteable<&WalletCapability> for T
where
    T: ReceivedNoteAndMetadata,
{
    const VERSION: u8 = 4;

    fn read<R: Read>(mut reader: R, wallet_capability: &WalletCapability) -> io::Result<Self> {
        let external_version = Self::get_version(&mut reader)?;
        tracing::info!("NoteAndMetadata version is: {external_version}");

        if external_version < 2 {
            let mut x = <T as ReceivedNoteAndMetadata>::get_deprecated_serialized_view_key_buffer();
            reader.read_exact(&mut x).expect("To not used this data.");
        }

        let mut diversifier_bytes = [0u8; 11];
        reader.read_exact(&mut diversifier_bytes)?;
        let diversifier = T::Diversifier::from_bytes(diversifier_bytes);

        let note =
            <T::Note as ReadableWriteable<_>>::read(&mut reader, (diversifier, wallet_capability))?;

        let witnesses_vec = Vector::read(&mut reader, |r| read_incremental_witness(r))?;
        let top_height = reader.read_u64::<LittleEndian>()?;
        let witnesses = WitnessCache::<T::Node>::new(witnesses_vec, top_height);

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

        if external_version < 3 {
            let _unconfirmed_spent = {
                Optional::read(&mut reader, |r| {
                    let mut transaction_bytes = [0u8; 32];
                    r.read_exact(&mut transaction_bytes)?;

                    let height = r.read_u32::<LittleEndian>()?;
                    Ok((TxId::from_bytes(transaction_bytes), height))
                })?
            };
        }

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

        let output_index = if external_version >= 4 {
            reader.read_u32::<LittleEndian>()?
        } else {
            // TODO: This value is obviously incorrect, we can fix it if it becomes a problem
            u32::MAX
        };

        Ok(T::from_parts(
            diversifier,
            note,
            witnesses.last().unwrap().witnessed_position(),
            Some(nullifier),
            spent,
            None,
            memo,
            is_change,
            have_spending_key,
            output_index as usize,
        ))
    }

    fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        // Write a version number first, so we can later upgrade this if needed.
        writer.write_u8(Self::VERSION)?;

        writer.write_all(&self.diversifier().to_bytes())?;

        self.note().write(&mut writer)?;
        writer.write_u64::<LittleEndian>(u64::from(*self.witnessed_position()))?;

        writer.write_all(&self.nullifier().to_bytes())?;

        Optional::write(
            &mut writer,
            self.spent().as_ref(),
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

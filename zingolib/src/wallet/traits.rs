//! Provides unifying interfaces for transaction management across Sapling and Orchard
use crate::wallet::notes::interface::OutputConstructor;
use std::io::{self, Read, Write};

use crate::data::witness_trees::WitnessTrees;
use crate::wallet::notes::OutputInterface;
use crate::wallet::notes::ShieldedNoteInterface;
use crate::wallet::{
    data::{
        PoolNullifier, SpendableOrchardNote, SpendableSaplingNote, TransactionRecord, WitnessCache,
        COMMITMENT_TREE_LEVELS, MAX_SHARD_LEVEL,
    },
    keys::unified::WalletCapability,
    notes::{OrchardNote, SaplingNote},
    tx_map_and_maybe_trees::TxMapAndMaybeTrees,
};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use incrementalmerkletree::{witness::IncrementalWitness, Hashable, Level, Position};
use nonempty::NonEmpty;
use orchard::{
    note_encryption::{CompactAction, OrchardDomain},
    primitives::redpallas::{Signature, SpendAuth},
    tree::MerkleHashOrchard,
    Action,
};
use sapling_crypto::{bundle::GrothProofBytes, note_encryption::SaplingDomain};
use shardtree::store::memory::MemoryShardStore;
use shardtree::ShardTree;
use subtle::CtOption;
use zcash_address::unified::{self, Receiver};
use zcash_client_backend::{
    address::UnifiedAddress,
    encoding::encode_payment_address,
    proto::{
        compact_formats::{CompactOrchardAction, CompactSaplingOutput, CompactTx},
        service::TreeState,
    },
    ShieldedProtocol,
};
use zcash_encoding::{Optional, Vector};
use zcash_note_encryption::{
    BatchDomain, Domain, EphemeralKeyBytes, ShieldedOutput, COMPACT_NOTE_SIZE, ENC_CIPHERTEXT_SIZE,
};
use zcash_primitives::{
    consensus::{BlockHeight, NetworkConstants, NetworkUpgrade, Parameters},
    memo::{Memo, MemoBytes},
    merkle_tree::read_incremental_witness,
    transaction::{
        components::{Amount, OutputDescription, SpendDescription},
        Transaction, TxId,
    },
};
use zingoconfig::ChainType;

/// This provides a uniform `.to_bytes` to types that might require it in a generic context.
pub trait ToBytes<const N: usize> {
    /// TODO: Add Doc Comment Here!
    fn to_bytes(&self) -> [u8; N];
}

impl ToBytes<32> for sapling_crypto::Nullifier {
    fn to_bytes(&self) -> [u8; 32] {
        self.0
    }
}

impl ToBytes<32> for orchard::note::Nullifier {
    fn to_bytes(&self) -> [u8; 32] {
        orchard::note::Nullifier::to_bytes(*self)
    }
}

impl ToBytes<11> for sapling_crypto::Diversifier {
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
    /// TODO: Add Doc Comment Here!
    fn domain(&self, height: BlockHeight, parameters: ChainType) -> D;

    /// A decryption key for `enc_ciphertext`.  `out_ciphertext` is _itself_  decryptable
    /// with the `OutgoingCipherKey` "`ock`".
    fn out_ciphertext(&self) -> [u8; 80];

    /// TODO: Add Doc Comment Here!
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

impl ShieldedOutputExt<SaplingDomain> for OutputDescription<GrothProofBytes> {
    fn domain(&self, height: BlockHeight, parameters: ChainType) -> SaplingDomain {
        SaplingDomain::new(
            zcash_primitives::transaction::components::sapling::zip212_enforcement(
                &parameters,
                height,
            ),
        )
    }

    fn out_ciphertext(&self) -> [u8; 80] {
        *self.out_ciphertext()
    }

    fn value_commitment(&self) -> <SaplingDomain as Domain>::ValueCommitment {
        self.cv().clone()
    }
}

/// Provides a standard `from_bytes` interface to be used generically
pub trait FromBytes<const N: usize> {
    /// TODO: Add Doc Comment Here!
    fn from_bytes(bytes: [u8; N]) -> Self;
}

impl FromBytes<32> for sapling_crypto::Nullifier {
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

impl FromBytes<11> for sapling_crypto::Diversifier {
    fn from_bytes(bytes: [u8; 11]) -> Self {
        sapling_crypto::Diversifier(bytes)
    }
}

impl FromBytes<11> for orchard::keys::Diversifier {
    fn from_bytes(bytes: [u8; 11]) -> Self {
        orchard::keys::Diversifier::from_bytes(bytes)
    }
}

/// TODO: Add Doc Comment Here!
pub trait FromCommitment
where
    Self: Sized,
{
    /// TODO: Add Doc Comment Here!
    fn from_commitment(from: &[u8; 32]) -> CtOption<Self>;
}

impl FromCommitment for sapling_crypto::Node {
    fn from_commitment(from: &[u8; 32]) -> CtOption<Self> {
        let maybe_node =
            <sapling_crypto::Node as zcash_primitives::merkle_tree::HashSer>::read(from.as_slice());
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
    /// TODO: Add Doc Comment Here!
    type Nullifier: Nullifier;

    /// TODO: Add Doc Comment Here!
    fn nullifier(&self) -> &Self::Nullifier;
}

impl<Auth: sapling_crypto::bundle::Authorization> Spend for SpendDescription<Auth> {
    type Nullifier = sapling_crypto::Nullifier;
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

impl From<sapling_crypto::Nullifier> for PoolNullifier {
    fn from(n: sapling_crypto::Nullifier) -> Self {
        PoolNullifier::Sapling(n)
    }
}

///  Recipients provide the means to generate a Receiver.  A Receiver contains the information necessary
///  to transfer an asset to the generating Recipient.
///  <https://zips.z.cash/zip-0316#terminology>
pub trait Recipient {
    /// TODO: Add Doc Comment Here!
    type Diversifier: Copy;

    /// TODO: Add Doc Comment Here!
    fn diversifier(&self) -> Self::Diversifier;

    /// TODO: Add Doc Comment Here!
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
            &chain.network_type(),
        )
    }
}

impl Recipient for sapling_crypto::PaymentAddress {
    type Diversifier = sapling_crypto::Diversifier;

    fn diversifier(&self) -> Self::Diversifier {
        *sapling_crypto::PaymentAddress::diversifier(self)
    }

    fn b32encode_for_network(&self, chain: &ChainType) -> String {
        encode_payment_address(chain.hrp_sapling_payment_address(), self)
    }
}

fn slice_to_array<const N: usize>(slice: &[u8]) -> &[u8; N] {
    <&[u8; N]>::try_from(slice).unwrap_or(&[0; N])
    //todo: This default feels dangerous. Find better solution
}

/// TODO: Add Doc Comment Here!
pub trait CompactOutput<D: DomainWalletExt>: Sized + Clone
where
    D::Recipient: Recipient,
    <D as Domain>::Note: PartialEq + Clone,
{
    /// TODO: Add Doc Comment Here!
    type CompactAction: ShieldedOutput<D, COMPACT_NOTE_SIZE>;

    /// TODO: Add Doc Comment Here!
    fn from_compact_transaction(compact_transaction: &CompactTx) -> &Vec<Self>;

    /// TODO: Add Doc Comment Here!
    fn cmstar(&self) -> &[u8; 32];

    /// TODO: Add Doc Comment Here!
    fn domain(&self, parameters: ChainType, height: BlockHeight) -> D;

    /// TODO: Add Doc Comment Here!
    fn to_compact_output_impl(&self) -> Self::CompactAction;
}

impl CompactOutput<SaplingDomain> for CompactSaplingOutput {
    type CompactAction = sapling_crypto::note_encryption::CompactOutputDescription;
    fn from_compact_transaction(compact_transaction: &CompactTx) -> &Vec<CompactSaplingOutput> {
        &compact_transaction.outputs
    }

    fn cmstar(&self) -> &[u8; 32] {
        slice_to_array(&self.cmu)
    }

    fn domain(&self, parameters: ChainType, height: BlockHeight) -> SaplingDomain {
        SaplingDomain::new(
            zcash_primitives::transaction::components::sapling::zip212_enforcement(
                &parameters,
                height,
            ),
        )
    }

    fn to_compact_output_impl(&self) -> Self::CompactAction {
        self.clone().try_into().unwrap()
    }
}

impl CompactOutput<OrchardDomain> for CompactOrchardAction {
    type CompactAction = CompactAction;
    fn from_compact_transaction(compact_transaction: &CompactTx) -> &Vec<CompactOrchardAction> {
        &compact_transaction.actions
    }
    fn cmstar(&self) -> &[u8; 32] {
        slice_to_array(&self.cmx)
    }

    fn domain(&self, _parameters: ChainType, _heightt: BlockHeight) -> OrchardDomain {
        OrchardDomain::for_compact_action(&self.to_compact_output_impl())
    }

    fn to_compact_output_impl(&self) -> Self::CompactAction {
        CompactAction::from_parts(
            orchard::note::Nullifier::from_bytes(slice_to_array(&self.nullifier)).unwrap(),
            orchard::note::ExtractedNoteCommitment::from_bytes(slice_to_array(&self.cmx)).unwrap(),
            EphemeralKeyBytes(*slice_to_array(&self.ephemeral_key)),
            *slice_to_array(&self.ciphertext),
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
    /// TODO: Add Doc Comment Here!
    type Spends<'a>: IntoIterator<Item = &'a Self::Spend>
    where
        Self::Spend: 'a,
        Self: 'a;
    /// TODO: Add Doc Comment Here!
    type Outputs<'a>: IntoIterator<Item = &'a Self::Output>
    where
        Self::Output: 'a,
        Self: 'a;
    /// An extractive process that returns domain specific information from a transaction.
    fn from_transaction(transaction: &Transaction) -> Option<&Self>;

    /// Some domains, Orchard for example, do not expose
    /// immediately expose outputs
    fn output_elements(&self) -> Self::Outputs<'_>;

    /// TODO: Add Doc Comment Here!
    fn spend_elements(&self) -> Self::Spends<'_>;
}

impl Bundle<SaplingDomain> for sapling_crypto::Bundle<sapling_crypto::bundle::Authorized, Amount> {
    type Spend = SpendDescription<sapling_crypto::bundle::Authorized>;
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
    /// TODO: Add Doc Comment Here!
    fn get_nullifiers_spent_in_transaction(transaction: &TransactionRecord) -> &Vec<Self>;
}

impl Nullifier for sapling_crypto::Nullifier {
    fn get_nullifiers_spent_in_transaction(
        transaction_metadata_set: &TransactionRecord,
    ) -> &Vec<Self> {
        &transaction_metadata_set.spent_sapling_nullifiers
    }
}

impl Nullifier for orchard::note::Nullifier {
    fn get_nullifiers_spent_in_transaction(transaction: &TransactionRecord) -> &Vec<Self> {
        &transaction.spent_orchard_nullifiers
    }
}

type MemoryStoreShardTree<T> =
    ShardTree<MemoryShardStore<T, BlockHeight>, COMMITMENT_TREE_LEVELS, MAX_SHARD_LEVEL>;

/// TODO: Add Doc Comment Here!
pub trait DomainWalletExt: Domain + BatchDomain
where
    Self: Sized,
    Self::Note: PartialEq + Clone,
    Self::Recipient: Recipient,
{
    /// TODO: Add Doc Comment Here!
    const NU: NetworkUpgrade;
    /// TODO: Add Doc Comment Here!
    const NAME: &'static str;
    /// The [zcash_client_backend::ShieldedProtocol] this domain represents
    const SHIELDED_PROTOCOL: ShieldedProtocol;

    /// TODO: Add Doc Comment Here!
    type Fvk: Clone
        + Send
        + Diversifiable<Note = Self::WalletNote, Address = Self::Recipient>
        + for<'a> TryFrom<&'a WalletCapability>
        + super::keys::unified::Fvk<Self>;

    /// TODO: Add Doc Comment Here!
    type SpendingKey: for<'a> TryFrom<&'a WalletCapability> + Clone;
    /// TODO: Add Doc Comment Here!
    type CompactOutput: CompactOutput<Self>;
    /// TODO: Add Doc Comment Here!
    type WalletNote: ShieldedNoteInterface<
        Note = <Self as Domain>::Note,
        Diversifier = <<Self as Domain>::Recipient as Recipient>::Diversifier,
        Nullifier = <<<Self as DomainWalletExt>::Bundle as Bundle<Self>>::Spend as Spend>::Nullifier,
    > + std::fmt::Debug;
    /// TODO: Add Doc Comment Here!
    type SpendableNoteAT: SpendableNote<Self>;
    /// TODO: Add Doc Comment Here!
    type Bundle: Bundle<Self>;

    /// TODO: Add Doc Comment Here!
    fn sum_pool_change(transaction_md: &TransactionRecord) -> u64 {
        Self::WalletNote::get_record_outputs(transaction_md)
            .iter()
            .filter(|nd| nd.is_change())
            .map(|nd| nd.value())
            .sum()
    }

    /// TODO: Add Doc Comment Here!
    fn transaction_metadata_set_to_shardtree(
        txmds: &TxMapAndMaybeTrees,
    ) -> Option<&MemoryStoreShardTree<<Self::WalletNote as ShieldedNoteInterface>::Node>> {
        txmds
            .witness_trees()
            .map(|trees| Self::get_shardtree(trees))
    }

    /// TODO: Add Doc Comment Here!
    fn transaction_metadata_set_to_shardtree_mut(
        txmds: &mut TxMapAndMaybeTrees,
    ) -> Option<&mut MemoryStoreShardTree<<Self::WalletNote as ShieldedNoteInterface>::Node>> {
        txmds
            .witness_trees_mut()
            .map(|trees| Self::get_shardtree_mut(trees))
    }

    /// TODO: Add Doc Comment Here!
    fn get_shardtree(
        trees: &WitnessTrees,
    ) -> &MemoryStoreShardTree<<Self::WalletNote as ShieldedNoteInterface>::Node>;

    /// TODO: Add Doc Comment Here!
    fn get_shardtree_mut(
        trees: &mut WitnessTrees,
    ) -> &mut MemoryStoreShardTree<<Self::WalletNote as ShieldedNoteInterface>::Node>;

    /// TODO: Add Doc Comment Here!
    fn get_nullifier_from_note_fvk_and_witness_position(
        note: &Self::Note,
        fvk: &Self::Fvk,
        position: u64,
    ) -> <Self::WalletNote as ShieldedNoteInterface>::Nullifier;

    /// TODO: Add Doc Comment Here!
    fn get_tree(tree_state: &TreeState) -> &String;

    /// TODO: Add Doc Comment Here!
    fn ua_from_contained_receiver<'a>(
        unified_spend_auth: &'a WalletCapability,
        receiver: &Self::Recipient,
    ) -> Option<&'a UnifiedAddress>;

    /// TODO: Add Doc Comment Here!
    fn wc_to_fvk(wc: &WalletCapability) -> Result<Self::Fvk, String>;

    /// TODO: Add Doc Comment Here!
    fn wc_to_sk(wc: &WalletCapability) -> Result<Self::SpendingKey, String>;
}

impl DomainWalletExt for SaplingDomain {
    const NU: NetworkUpgrade = NetworkUpgrade::Sapling;
    const NAME: &'static str = "sapling";
    const SHIELDED_PROTOCOL: ShieldedProtocol = ShieldedProtocol::Sapling;

    type Fvk = sapling_crypto::zip32::DiversifiableFullViewingKey;

    type SpendingKey = sapling_crypto::zip32::ExtendedSpendingKey;

    type CompactOutput = CompactSaplingOutput;

    type WalletNote = SaplingNote;

    type SpendableNoteAT = SpendableSaplingNote;

    type Bundle = sapling_crypto::Bundle<sapling_crypto::bundle::Authorized, Amount>;

    fn get_shardtree(
        trees: &WitnessTrees,
    ) -> &ShardTree<
        MemoryShardStore<<Self::WalletNote as ShieldedNoteInterface>::Node, BlockHeight>,
        COMMITMENT_TREE_LEVELS,
        MAX_SHARD_LEVEL,
    > {
        &trees.witness_tree_sapling
    }

    fn get_shardtree_mut(
        trees: &mut WitnessTrees,
    ) -> &mut ShardTree<
        MemoryShardStore<<Self::WalletNote as ShieldedNoteInterface>::Node, BlockHeight>,
        COMMITMENT_TREE_LEVELS,
        MAX_SHARD_LEVEL,
    > {
        &mut trees.witness_tree_sapling
    }

    fn get_nullifier_from_note_fvk_and_witness_position(
        note: &Self::Note,
        fvk: &Self::Fvk,
        position: u64,
    ) -> <<Self as DomainWalletExt>::WalletNote as ShieldedNoteInterface>::Nullifier {
        note.nf(&fvk.fvk().vk.nk, position)
    }

    fn get_tree(tree_state: &TreeState) -> &String {
        &tree_state.sapling_tree
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

    fn wc_to_sk(wc: &WalletCapability) -> Result<Self::SpendingKey, String> {
        Self::SpendingKey::try_from(wc)
    }
}

impl DomainWalletExt for OrchardDomain {
    const NU: NetworkUpgrade = NetworkUpgrade::Nu5;
    const NAME: &'static str = "orchard";
    const SHIELDED_PROTOCOL: ShieldedProtocol = ShieldedProtocol::Orchard;

    type Fvk = orchard::keys::FullViewingKey;

    type SpendingKey = orchard::keys::SpendingKey;

    type CompactOutput = CompactOrchardAction;

    type WalletNote = OrchardNote;

    type SpendableNoteAT = SpendableOrchardNote;

    type Bundle = orchard::bundle::Bundle<orchard::bundle::Authorized, Amount>;

    fn get_shardtree(
        trees: &WitnessTrees,
    ) -> &ShardTree<
        MemoryShardStore<<Self::WalletNote as ShieldedNoteInterface>::Node, BlockHeight>,
        COMMITMENT_TREE_LEVELS,
        MAX_SHARD_LEVEL,
    > {
        &trees.witness_tree_orchard
    }

    fn get_shardtree_mut(
        trees: &mut WitnessTrees,
    ) -> &mut ShardTree<
        MemoryShardStore<<Self::WalletNote as ShieldedNoteInterface>::Node, BlockHeight>,
        COMMITMENT_TREE_LEVELS,
        MAX_SHARD_LEVEL,
    > {
        &mut trees.witness_tree_orchard
    }

    fn get_nullifier_from_note_fvk_and_witness_position(
        note: &Self::Note,
        fvk: &Self::Fvk,
        _position: u64,
    ) -> <<Self as DomainWalletExt>::WalletNote as ShieldedNoteInterface>::Nullifier {
        note.nullifier(fvk)
    }

    fn get_tree(tree_state: &TreeState) -> &String {
        &tree_state.orchard_tree
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

    fn wc_to_sk(wc: &WalletCapability) -> Result<Self::SpendingKey, String> {
        Self::SpendingKey::try_from(wc)
    }
}

/// TODO: Add Doc Comment Here!
pub trait Diversifiable {
    /// TODO: Add Doc Comment Here!
    type Note: ShieldedNoteInterface;
    /// TODO: Add Doc Comment Here!
    type Address: Recipient;

    /// TODO: Add Doc Comment Here!
    fn diversified_address(
        &self,
        div: <Self::Note as ShieldedNoteInterface>::Diversifier,
    ) -> Option<Self::Address>;
}

impl Diversifiable for sapling_crypto::zip32::DiversifiableFullViewingKey {
    type Note = SaplingNote;

    type Address = sapling_crypto::PaymentAddress;

    fn diversified_address(
        &self,
        div: <<sapling_crypto::zip32::DiversifiableFullViewingKey as Diversifiable>::Note as ShieldedNoteInterface>::Diversifier,
    ) -> Option<Self::Address> {
        self.fvk().vk.to_payment_address(div)
    }
}

impl Diversifiable for orchard::keys::FullViewingKey {
    type Note = OrchardNote;
    type Address = orchard::Address;

    fn diversified_address(
        &self,
        div: <<orchard::keys::FullViewingKey as Diversifiable>::Note as ShieldedNoteInterface>::Diversifier,
    ) -> Option<Self::Address> {
        Some(self.address(div, orchard::keys::Scope::External))
    }
}

/// TODO: Add Doc Comment Here!
pub trait SpendableNote<D>
where
    D: DomainWalletExt<SpendableNoteAT = Self>,
    <D as Domain>::Recipient: Recipient,
    <D as Domain>::Note: PartialEq + Clone,
    Self: Sized,
{
    /// TODO: Add Doc Comment Here!
    fn from(
        transaction_id: TxId,
        note_and_metadata: &D::WalletNote,
        spend_key: Option<&D::SpendingKey>,
    ) -> Option<Self> {
        // Include only non-0 value notes that haven't been spent, or haven't been included
        // in an pending spend yet.
        if Self::check_spendability_of_note(note_and_metadata, spend_key) {
            // Filter out notes with nullifier or position not yet known
            if let (Some(nf), Some(pos)) = (
                note_and_metadata.nullifier(),
                note_and_metadata.witnessed_position(),
            ) {
                Some(Self::from_parts_unchecked(
                    transaction_id,
                    nf,
                    *note_and_metadata.diversifier(),
                    note_and_metadata.note().clone(),
                    *pos,
                    spend_key,
                ))
            } else {
                None
            }
        } else {
            None
        }
    }

    /// TODO: Add Doc Comment Here!
    fn check_spendability_of_note(
        note_and_metadata: &D::WalletNote,
        spend_key: Option<&D::SpendingKey>,
    ) -> bool {
        note_and_metadata.spent().is_none()
            && note_and_metadata.pending_spent().is_none()
            && spend_key.is_some()
            && note_and_metadata.value() != 0
    }

    /// The checks needed are shared between domains, and thus are performed in the
    /// default impl of `from`. This function's only caller should be `Self::from`
    fn from_parts_unchecked(
        transaction_id: TxId,
        nullifier: <D::WalletNote as ShieldedNoteInterface>::Nullifier,
        diversifier: <D::WalletNote as ShieldedNoteInterface>::Diversifier,
        note: D::Note,
        witnessed_position: Position,
        sk: Option<&D::SpendingKey>,
    ) -> Self;

    /// TODO: Add Doc Comment Here!
    fn transaction_id(&self) -> TxId;

    /// TODO: Add Doc Comment Here!
    fn nullifier(&self) -> <D::WalletNote as ShieldedNoteInterface>::Nullifier;

    /// TODO: Add Doc Comment Here!
    fn diversifier(&self) -> <D::WalletNote as ShieldedNoteInterface>::Diversifier;

    /// TODO: Add Doc Comment Here!
    fn note(&self) -> &D::Note;

    /// TODO: Add Doc Comment Here!
    fn witnessed_position(&self) -> &Position;

    /// TODO: Add Doc Comment Here!
    fn spend_key(&self) -> Option<&D::SpendingKey>;
}

impl SpendableNote<SaplingDomain> for SpendableSaplingNote {
    fn from_parts_unchecked(
        transaction_id: TxId,
        nullifier: sapling_crypto::Nullifier,
        diversifier: sapling_crypto::Diversifier,
        note: sapling_crypto::Note,
        witnessed_position: Position,
        extsk: Option<&sapling_crypto::zip32::ExtendedSpendingKey>,
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

    fn nullifier(&self) -> sapling_crypto::Nullifier {
        self.nullifier
    }

    fn diversifier(&self) -> sapling_crypto::Diversifier {
        self.diversifier
    }

    fn note(&self) -> &sapling_crypto::Note {
        &self.note
    }

    fn witnessed_position(&self) -> &Position {
        &self.witnessed_position
    }

    fn spend_key(&self) -> Option<&sapling_crypto::zip32::ExtendedSpendingKey> {
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

/// TODO: Add Doc Comment Here!
pub trait ReadableWriteable<Input>: Sized {
    /// TODO: Add Doc Comment Here!
    const VERSION: u8;

    /// TODO: Add Doc Comment Here!
    fn read<R: Read>(reader: R, input: Input) -> io::Result<Self>;

    /// TODO: Add Doc Comment Here!
    fn write<W: Write>(&self, writer: W) -> io::Result<()>;

    /// TODO: Add Doc Comment Here!
    fn get_version<R: Read>(mut reader: R) -> io::Result<u8> {
        let external_version = reader.read_u8()?;
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

impl ReadableWriteable<()> for sapling_crypto::zip32::ExtendedSpendingKey {
    const VERSION: u8 = 0; //Not applicable

    fn read<R: Read>(reader: R, _: ()) -> io::Result<Self> {
        Self::read(reader)
    }

    fn write<W: Write>(&self, writer: W) -> io::Result<()> {
        self.write(writer)
    }
}

impl ReadableWriteable<()> for sapling_crypto::zip32::DiversifiableFullViewingKey {
    const VERSION: u8 = 0; //Not applicable

    fn read<R: Read>(mut reader: R, _: ()) -> io::Result<Self> {
        let mut fvk_bytes = [0u8; 128];
        reader.read_exact(&mut fvk_bytes)?;
        sapling_crypto::zip32::DiversifiableFullViewingKey::from_bytes(&fvk_bytes).ok_or(
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Couldn't read a Sapling Diversifiable Full Viewing Key",
            ),
        )
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

impl ReadableWriteable<(sapling_crypto::Diversifier, &WalletCapability)> for sapling_crypto::Note {
    const VERSION: u8 = 1;

    fn read<R: Read>(
        mut reader: R,
        (diversifier, wallet_capability): (sapling_crypto::Diversifier, &WalletCapability),
    ) -> io::Result<Self> {
        let _version = Self::get_version(&mut reader)?;
        let value = reader.read_u64::<LittleEndian>()?;
        let rseed = super::data::read_sapling_rseed(&mut reader)?;

        Ok(
            <SaplingDomain as DomainWalletExt>::wc_to_fvk(wallet_capability)
                .expect("to get an fvk from a wc")
                .fvk()
                .vk
                .to_payment_address(diversifier)
                .unwrap()
                .create_note(sapling_crypto::value::NoteValue::from_raw(value), rseed),
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
        let rho_nullifier = Option::from(orchard::note::Rho::from_bytes(&nullifier_bytes))
            .ok_or(io::Error::new(io::ErrorKind::InvalidInput, "Bad Nullifier"))?;

        let mut random_seed_bytes = [0; 32];
        reader.read_exact(&mut random_seed_bytes)?;
        let random_seed = Option::from(orchard::note::RandomSeed::from_bytes(
            random_seed_bytes,
            &rho_nullifier,
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
            rho_nullifier,
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

impl<T>
    ReadableWriteable<(
        &WalletCapability,
        Option<
            &mut Vec<(
                IncrementalWitness<T::Node, COMMITMENT_TREE_LEVELS>,
                BlockHeight,
            )>,
        >,
    )> for T
where
    T: ShieldedNoteInterface,
{
    const VERSION: u8 = 4;

    fn read<R: Read>(
        mut reader: R,
        (wallet_capability, inc_wit_vec): (
            &WalletCapability,
            Option<
                &mut Vec<(
                    IncrementalWitness<T::Node, COMMITMENT_TREE_LEVELS>,
                    BlockHeight,
                )>,
            >,
        ),
    ) -> io::Result<Self> {
        let external_version = Self::get_version(&mut reader)?;

        if external_version < 2 {
            let mut x = <T as ShieldedNoteInterface>::get_deprecated_serialized_view_key_buffer();
            reader.read_exact(&mut x).expect("To not used this data.");
        }

        let mut diversifier_bytes = [0u8; 11];
        reader.read_exact(&mut diversifier_bytes)?;
        let diversifier = T::Diversifier::from_bytes(diversifier_bytes);

        let note =
            <T::Note as ReadableWriteable<_>>::read(&mut reader, (diversifier, wallet_capability))?;

        let witnessed_position = if external_version >= 4 {
            Position::from(reader.read_u64::<LittleEndian>()?)
        } else {
            let witnesses_vec = Vector::read(&mut reader, |r| read_incremental_witness(r))?;

            let top_height = reader.read_u64::<LittleEndian>()?;
            let witnesses = WitnessCache::<T::Node>::new(witnesses_vec, top_height);

            let pos = witnesses
                .last()
                .map(|w| w.witnessed_position())
                .unwrap_or_else(|| Position::from(0));
            for (i, witness) in witnesses.witnesses.into_iter().rev().enumerate().rev() {
                let height = BlockHeight::from(top_height as u32 - i as u32);
                if let Some(&mut ref mut wits) = inc_wit_vec {
                    wits.push((witness, height));
                }
            }
            pos
        };

        let mut nullifier = [0u8; 32];
        reader.read_exact(&mut nullifier)?;
        let nullifier = T::Nullifier::from_bytes(nullifier);

        // Note that this is only the spent field, we ignore the pending_spent field.
        // The reason is that pending spents are only in memory, and we need to get the actual value of spent
        // from the blockchain anyway.
        let spent = Optional::read(&mut reader, |r| {
            let mut transaction_id_bytes = [0u8; 32];
            r.read_exact(&mut transaction_id_bytes)?;
            let height = r.read_u32::<LittleEndian>()?;
            Ok((TxId::from_bytes(transaction_id_bytes), height))
        })?;

        if external_version < 3 {
            let _pending_spent = {
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
            match reader.read_u32::<LittleEndian>()? {
                u32::MAX => None,
                otherwise => Some(otherwise),
            }
        } else {
            None
        };

        Ok(T::from_parts(
            diversifier,
            note,
            Some(witnessed_position),
            Some(nullifier),
            spent,
            None,
            memo,
            is_change,
            have_spending_key,
            output_index,
        ))
    }

    fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        // Write a version number first, so we can later upgrade this if needed.
        writer.write_u8(Self::VERSION)?;

        writer.write_all(&self.diversifier().to_bytes())?;

        self.note().write(&mut writer)?;
        writer.write_u64::<LittleEndian>(u64::from(self.witnessed_position().ok_or(
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Tried to write note without knowing its the position of its value commitment",
            ),
        )?))?;

        writer.write_all(
            &self
                .nullifier()
                .ok_or(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Tried to write note with unknown nullifier",
                ))?
                .to_bytes(),
        )?;

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

        writer.write_u32::<LittleEndian>(self.output_index().unwrap_or(u32::MAX))?;

        Ok(())
    }
}

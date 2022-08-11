//! Provides unifying interfaces for transaction management across Sapling and Orchard
use std::io::{self, Read, Write};

use super::{
    data::{
        OrchardNoteAndMetadata, SaplingNoteAndMetadata, TransactionMetadata, WalletNullifier,
        WitnessCache,
    },
    keys::{orchard::OrchardKey, sapling::SaplingKey, Keys},
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
        IncomingViewingKey as OrchardIncomingViewingKey,
        OutgoingViewingKey as OrchardOutgoingViewingKey, SpendingKey as OrchardSpendingKey,
    },
    note::{Note as OrchardNote, Nullifier as OrchardNullifier},
    note_encryption::OrchardDomain,
    primitives::redpallas::{Signature, SpendAuth},
    tree::MerkleHashOrchard,
    Action, Address as OrchardAddress,
};
use subtle::CtOption;
use zcash_address::unified::{self, Encoding as _, Receiver};
use zcash_client_backend::encoding::encode_payment_address;
use zcash_encoding::{Optional, Vector};
use zcash_note_encryption::{Domain, ShieldedOutput, ENC_CIPHERTEXT_SIZE};
use zcash_primitives::{
    consensus::{BlockHeight, Parameters},
    keys::OutgoingViewingKey as SaplingOutgoingViewingKey,
    memo::{Memo, MemoBytes},
    merkle_tree::{Hashable, IncrementalWitness},
    sapling::{
        note_encryption::SaplingDomain, Diversifier as SaplingDiversifier, Node as SaplingNode,
        Note as SaplingNote, Nullifier as SaplingNullifier, PaymentAddress as SaplingAddress,
        SaplingIvk,
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
use zingoconfig::Network;

/// This provides a uniform `.to_bytes` to types that might require it in a generic context.
pub(crate) trait ToBytes<const N: usize> {
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
pub(crate) trait ShieldedOutputExt<P: Parameters, D: Domain>:
    ShieldedOutput<D, ENC_CIPHERTEXT_SIZE>
{
    fn domain(&self, height: BlockHeight, parameters: P) -> D;
    /// A decryption key for `enc_ciphertext`.  `out_ciphertext` is _itself_  decryptable
    /// with the `OutgoingCipherKey` "`ock`".
    fn out_ciphertext(&self) -> [u8; 80];
    fn value_commitment(&self) -> D::ValueCommitment;
}

impl<A, P: Parameters> ShieldedOutputExt<P, OrchardDomain> for Action<A> {
    fn domain(&self, _block_height: BlockHeight, _parameters: P) -> OrchardDomain {
        OrchardDomain::for_action(self)
    }

    fn out_ciphertext(&self) -> [u8; 80] {
        self.encrypted_note().out_ciphertext
    }

    fn value_commitment(&self) -> orchard::value::ValueCommitment {
        self.cv_net().clone()
    }
}

impl<P: Parameters> ShieldedOutputExt<P, SaplingDomain<P>> for OutputDescription<GrothProofBytes> {
    fn domain(&self, height: BlockHeight, parameters: P) -> SaplingDomain<P> {
        SaplingDomain::for_height(parameters, height)
    }

    fn out_ciphertext(&self) -> [u8; 80] {
        self.out_ciphertext
    }

    fn value_commitment(&self) -> <SaplingDomain<Network> as Domain>::ValueCommitment {
        self.cv
    }
}

/// Provides a standard `from_bytes` interface to be used generically
pub(crate) trait FromBytes<const N: usize> {
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

pub(crate) trait FromCommitment
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
pub(crate) trait Spend {
    type Nullifier: Nullifier;
    fn nullifier(&self) -> &Self::Nullifier;
    fn wallet_nullifier(_: &Self::Nullifier) -> WalletNullifier;
}

impl<Auth: SaplingAuthorization> Spend for SpendDescription<Auth> {
    type Nullifier = SaplingNullifier;
    fn nullifier(&self) -> &Self::Nullifier {
        &self.nullifier
    }
    fn wallet_nullifier(null: &Self::Nullifier) -> WalletNullifier {
        WalletNullifier::Sapling(*null)
    }
}

impl<Auth> Spend for Action<Auth> {
    type Nullifier = OrchardNullifier;
    fn nullifier(&self) -> &Self::Nullifier {
        self.nullifier()
    }
    fn wallet_nullifier(null: &Self::Nullifier) -> WalletNullifier {
        WalletNullifier::Orchard(*null)
    }
}

///  Recipients provide the means to generate a Receiver.  A Receiver contains the information necessary
///  to transfer an asset to the generating Recipient.
///  <https://zips.z.cash/zip-0316#terminology>
pub(crate) trait Recipient {
    type Diversifier;
    fn diversifier(&self) -> Self::Diversifier;
    fn b32encode_for_network(&self, chain: &Network) -> String;
}

impl Recipient for OrchardAddress {
    type Diversifier = OrchardDiversifier;

    fn diversifier(&self) -> Self::Diversifier {
        OrchardAddress::diversifier(&self)
    }

    fn b32encode_for_network(&self, chain: &Network) -> String {
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

    fn b32encode_for_network(&self, chain: &Network) -> String {
        encode_payment_address(chain.hrp_sapling_payment_address(), self)
    }
}

pub(crate) trait CompactOutput: Sized {
    fn from_compact_transaction(compact_transaction: &CompactTx) -> &Vec<Self>;
    fn cmstar(&self) -> &[u8; 32];
}

impl CompactOutput for CompactSaplingOutput {
    fn from_compact_transaction(compact_transaction: &CompactTx) -> &Vec<CompactSaplingOutput> {
        &compact_transaction.outputs
    }

    fn cmstar(&self) -> &[u8; 32] {
        vec_to_array(&self.cmu)
    }
}

impl CompactOutput for CompactOrchardAction {
    fn from_compact_transaction(compact_transaction: &CompactTx) -> &Vec<CompactOrchardAction> {
        &compact_transaction.actions
    }
    fn cmstar(&self) -> &[u8; 32] {
        vec_to_array(&self.cmx)
    }
}

/// A set of transmission abstractions within a transaction, that are specific to a particular
/// domain. In the Orchard Domain bundles comprise Actions each of which contains
/// both a Spend and an Output (though either or both may be dummies). Sapling transmissions,
/// as implemented, contain a 1:1 ratio of Spends and Outputs.
pub(crate) trait Bundle<D: DomainWalletExt<P>, P: Parameters>
where
    D::Recipient: Recipient,
    D::Note: PartialEq,
{
    /// An expenditure of an ?external? output, such that its value is distributed among *this* transaction's outputs.
    type Spend: Spend;
    /// A value store that is completely emptied by transfer of its contents to another output.
    type Output: ShieldedOutputExt<P, D>;
    type Spends: IntoIterator<Item = Self::Spend>;
    type Outputs: IntoIterator<Item = Self::Output>;
    /// An extractive process that returns domain specific information from a transaction.
    fn from_transaction(transaction: &Transaction) -> Option<&Self>;
    fn outputs(&self) -> &Self::Outputs;
    fn spends(&self) -> &Self::Spends;
}

impl<P: Parameters> Bundle<SaplingDomain<P>, P> for SaplingBundle<SaplingAuthorized> {
    type Spend = SpendDescription<SaplingAuthorized>;
    type Output = OutputDescription<GrothProofBytes>;
    type Spends = Vec<Self::Spend>;
    type Outputs = Vec<Self::Output>;
    fn from_transaction(transaction: &Transaction) -> Option<&Self> {
        transaction.sapling_bundle()
    }

    fn outputs(&self) -> &Self::Outputs {
        &self.shielded_outputs
    }

    fn spends(&self) -> &Self::Spends {
        &self.shielded_spends
    }
}

impl<P: Parameters> Bundle<OrchardDomain, P> for OrchardBundle<OrchardAuthorized, Amount> {
    type Spend = Action<Signature<SpendAuth>>;
    type Output = Action<Signature<SpendAuth>>;
    type Spends = NonEmpty<Self::Spend>;
    type Outputs = NonEmpty<Self::Output>;

    fn from_transaction(transaction: &Transaction) -> Option<&Self> {
        transaction.orchard_bundle()
    }

    fn outputs(&self) -> &Self::Outputs {
        //! In orchard each action contains an output and a spend.
        self.actions()
    }

    fn spends(&self) -> &Self::Spends {
        //! In orchard each action contains an output and a spend.
        self.actions()
    }
}

/// TODO: Documentation neeeeeds help!!!!  XXXX
pub(crate) trait Nullifier: PartialEq + Copy + Sized + ToBytes<32> + FromBytes<32> {
    fn get_nullifiers_of_unspent_notes_from_transaction_set(
        transaction_metadata_set: &TransactionMetadataSet,
    ) -> Vec<(Self, u64, TxId)>;
    fn get_nullifiers_spent_in_transaction(transaction: &TransactionMetadata) -> &Vec<Self>;
    fn to_wallet_nullifier(&self) -> WalletNullifier;
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

    fn to_wallet_nullifier(&self) -> WalletNullifier {
        WalletNullifier::Sapling(*self)
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

    fn to_wallet_nullifier(&self) -> WalletNullifier {
        WalletNullifier::Orchard(*self)
    }
}

pub(crate) trait NoteAndMetadata: Sized {
    type Fvk: Clone + Diversifiable + ReadableWriteable<()>;
    type Diversifier: Copy + FromBytes<11> + ToBytes<11>;
    type Note: PartialEq + ReadableWriteable<(Self::Fvk, Self::Diversifier)>;
    type Node: Hashable + FromCommitment;
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
    fn diversifier(&self) -> &<<Self::Fvk as Diversifiable>::Note as NoteAndMetadata>::Diversifier;
    fn memo(&self) -> &Option<Memo>;
    fn memo_mut(&mut self) -> &mut Option<Memo>;
    fn note(&self) -> &Self::Note;
    fn nullifier(&self) -> Self::Nullifier;
    fn value(note: &Self::Note) -> u64;
    fn spent(&self) -> &Option<(TxId, u32)>;
    fn spent_mut(&mut self) -> &mut Option<(TxId, u32)>;
    fn unconfirmed_spent(&self) -> &Option<(TxId, u32)>;
    fn witnesses(&self) -> &WitnessCache<Self::Node>;
    fn witnesses_mut(&mut self) -> &mut WitnessCache<Self::Node>;
    fn have_spending_key(&self) -> bool;
    fn transaction_metadata_notes(wallet_transaction: &TransactionMetadata) -> &Vec<Self>;
    fn transaction_metadata_notes_mut(
        wallet_transaction: &mut TransactionMetadata,
    ) -> &mut Vec<Self>;
}

impl NoteAndMetadata for SaplingNoteAndMetadata {
    type Fvk = SaplingExtendedFullViewingKey;
    type Diversifier = SaplingDiversifier;
    type Note = SaplingNote;
    type Node = SaplingNode;
    type Nullifier = SaplingNullifier;

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

    fn nullifier(&self) -> Self::Nullifier {
        self.nullifier
    }

    fn value(note: &Self::Note) -> u64 {
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

impl NoteAndMetadata for OrchardNoteAndMetadata {
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

    fn nullifier(&self) -> Self::Nullifier {
        self.nullifier
    }

    fn value(note: &Self::Note) -> u64 {
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

/// A cross Domain interface to the hierarchy of capabilities deriveable from a SpendKey
pub(crate) trait WalletKey
where
    Self: Sized,
{
    type Sk;
    type Fvk;
    type Ivk;
    type Ovk;
    type Address;
    fn sk(&self) -> Option<Self::Sk>;
    fn fvk(&self) -> Option<Self::Fvk>;
    fn ivk(&self) -> Option<Self::Ivk>;
    fn ovk(&self) -> Option<Self::Ovk>;
    fn address(&self) -> Self::Address;
    fn addresses_from_keys(keys: &Keys) -> Vec<String>;
    fn get_keys(keys: &Keys) -> &Vec<Self>;
    fn set_spend_key_for_view_key(&mut self, key: Self::Sk);
}

impl WalletKey for SaplingKey {
    type Sk = SaplingExtendedSpendingKey;

    type Fvk = SaplingExtendedFullViewingKey;

    type Ivk = SaplingIvk;

    type Ovk = SaplingOutgoingViewingKey;

    type Address = SaplingAddress;

    fn sk(&self) -> Option<Self::Sk> {
        self.extsk.clone()
    }

    fn fvk(&self) -> Option<Self::Fvk> {
        Some(self.extfvk.clone())
    }

    fn ivk(&self) -> Option<Self::Ivk> {
        Some(self.extfvk.fvk.vk.ivk())
    }
    fn ovk(&self) -> Option<Self::Ovk> {
        Some(self.extfvk.fvk.ovk)
    }
    fn address(&self) -> Self::Address {
        self.zaddress.clone()
    }

    fn addresses_from_keys(keys: &Keys) -> Vec<String> {
        keys.get_all_sapling_addresses()
    }

    fn get_keys(keys: &Keys) -> &Vec<Self> {
        keys.zkeys()
    }

    fn set_spend_key_for_view_key(&mut self, key: Self::Sk) {
        self.extsk = Some(key);
        self.keytype = super::keys::sapling::WalletZKeyType::ImportedSpendingKey;
    }
}

impl WalletKey for OrchardKey {
    type Sk = OrchardSpendingKey;

    type Fvk = OrchardFullViewingKey;

    type Ivk = OrchardIncomingViewingKey;

    type Ovk = OrchardOutgoingViewingKey;

    type Address = zcash_client_backend::address::UnifiedAddress;

    fn sk(&self) -> Option<Self::Sk> {
        (&self.key).try_into().ok()
    }
    fn fvk(&self) -> Option<Self::Fvk> {
        (&self.key).try_into().ok()
    }
    fn ivk(&self) -> Option<Self::Ivk> {
        (&self.key).try_into().ok()
    }

    fn ovk(&self) -> Option<Self::Ovk> {
        (&self.key).try_into().ok()
    }

    fn address(&self) -> Self::Address {
        self.unified_address.clone()
    }

    fn addresses_from_keys(keys: &Keys) -> Vec<String> {
        keys.get_all_orchard_addresses()
    }

    fn get_keys(keys: &Keys) -> &Vec<Self> {
        keys.okeys()
    }

    fn set_spend_key_for_view_key(&mut self, key: Self::Sk) {
        self.key = super::keys::orchard::WalletOKeyInner::ImportedSpendingKey(key)
    }
}

pub(crate) trait DomainWalletExt<P: Parameters>: Domain
where
    Self: Sized,
    Self::Note: PartialEq,
    Self::Recipient: Recipient,
    Self::Note: PartialEq,
{
    type Fvk: Clone;
    type CompactOutput: CompactOutput;
    type WalletNote: NoteAndMetadata<
        Fvk = Self::Fvk,
        Note = <Self as Domain>::Note,
        Diversifier = <<Self as Domain>::Recipient as Recipient>::Diversifier,
        Nullifier = <<<Self as DomainWalletExt<P>>::Bundle as Bundle<Self, P>>::Spend as Spend>::Nullifier,
    >;
    type Key: WalletKey<
            Ovk = <Self as Domain>::OutgoingViewingKey,
            Ivk = <Self as Domain>::IncomingViewingKey,
            Fvk = <Self as DomainWalletExt<P>>::Fvk,
        > + Clone;

    type Bundle: Bundle<Self, P>;

    fn wallet_notes_mut(_: &mut TransactionMetadata) -> &mut Vec<Self::WalletNote>;
    fn get_tree(tree_state: &TreeState) -> &String;
}

impl<P: Parameters> DomainWalletExt<P> for SaplingDomain<P> {
    type Fvk = SaplingExtendedFullViewingKey;

    type CompactOutput = CompactSaplingOutput;

    type WalletNote = SaplingNoteAndMetadata;

    type Key = SaplingKey;

    type Bundle = SaplingBundle<SaplingAuthorized>;

    fn wallet_notes_mut(transaction: &mut TransactionMetadata) -> &mut Vec<Self::WalletNote> {
        &mut transaction.sapling_notes
    }

    fn get_tree(tree_state: &TreeState) -> &String {
        &tree_state.sapling_tree
    }
}

impl<P: Parameters> DomainWalletExt<P> for OrchardDomain {
    type Fvk = OrchardFullViewingKey;

    type CompactOutput = CompactOrchardAction;

    type WalletNote = OrchardNoteAndMetadata;

    type Key = OrchardKey;

    type Bundle = OrchardBundle<OrchardAuthorized, Amount>;

    fn wallet_notes_mut(transaction: &mut TransactionMetadata) -> &mut Vec<Self::WalletNote> {
        &mut transaction.orchard_notes
    }

    fn get_tree(tree_state: &TreeState) -> &String {
        &tree_state.orchard_tree
    }
}

pub(crate) trait Diversifiable {
    type Note: NoteAndMetadata;
    type Address: Recipient;
    fn diversified_address(
        &self,
        div: <Self::Note as NoteAndMetadata>::Diversifier,
    ) -> Option<Self::Address>;
}

impl Diversifiable for SaplingExtendedFullViewingKey {
    type Note = SaplingNoteAndMetadata;

    type Address = zcash_primitives::sapling::PaymentAddress;

    fn diversified_address(
        &self,
        div: <<zcash_primitives::zip32::ExtendedFullViewingKey as Diversifiable>::Note as NoteAndMetadata>::Diversifier,
    ) -> Option<Self::Address> {
        self.fvk.vk.to_payment_address(div)
    }
}

impl Diversifiable for OrchardFullViewingKey {
    type Note = OrchardNoteAndMetadata;
    type Address = orchard::Address;

    fn diversified_address(
        &self,
        div: <<orchard::keys::FullViewingKey as Diversifiable>::Note as NoteAndMetadata>::Diversifier,
    ) -> Option<Self::Address> {
        Some(self.address(div, orchard::keys::Scope::External))
    }
}

pub trait ReadableWriteable<Input>: Sized {
    type VersionSize;
    const VERSION: Self::VersionSize;

    fn read<R: Read>(reader: R, input: Input) -> io::Result<Self>;
    fn write<W: Write>(&self, writer: W) -> io::Result<()>;
}

impl ReadableWriteable<()> for SaplingExtendedFullViewingKey {
    type VersionSize = ();

    const VERSION: Self::VersionSize = (); //Not applicable

    fn read<R: Read>(reader: R, _: ()) -> io::Result<Self> {
        Self::read(reader)
    }

    fn write<W: Write>(&self, writer: W) -> io::Result<()> {
        self.write(writer)
    }
}

impl ReadableWriteable<()> for OrchardFullViewingKey {
    type VersionSize = ();

    const VERSION: Self::VersionSize = (); //Not applicable

    fn read<R: Read>(reader: R, _: ()) -> io::Result<Self> {
        Self::read(reader)
    }

    fn write<W: Write>(&self, writer: W) -> io::Result<()> {
        self.write(writer)
    }
}

impl ReadableWriteable<(SaplingExtendedFullViewingKey, SaplingDiversifier)> for SaplingNote {
    type VersionSize = ();

    const VERSION: Self::VersionSize = ();

    fn read<R: Read>(
        mut reader: R,
        (extfvk, diversifier): (SaplingExtendedFullViewingKey, SaplingDiversifier),
    ) -> io::Result<Self> {
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
        writer.write_u64::<LittleEndian>(self.value)?;
        super::data::write_sapling_rseed(&mut writer, &self.rseed)?;
        Ok(())
    }
}

impl ReadableWriteable<(OrchardFullViewingKey, OrchardDiversifier)> for OrchardNote {
    type VersionSize = ();

    const VERSION: Self::VersionSize = ();

    fn read<R: Read>(
        mut reader: R,
        (fvk, diversifier): (OrchardFullViewingKey, OrchardDiversifier),
    ) -> io::Result<Self> {
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

        Ok(OrchardNote::from_parts(
            fvk.address(diversifier, orchard::keys::Scope::External),
            orchard::value::NoteValue::from_raw(value),
            nullifier,
            random_seed,
        ))
    }

    fn write<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_u64::<LittleEndian>(self.value().inner())?;
        writer.write_all(&self.rho().to_bytes())?;
        writer.write_all(self.random_seed().as_bytes())?;
        Ok(())
    }
}

impl<T> ReadableWriteable<()> for T
where
    T: NoteAndMetadata,
{
    type VersionSize = u64; //I don't know why this is so big, but it changing it would break reading
                            //of old versios

    const VERSION: Self::VersionSize = 20;

    fn read<R: Read>(mut reader: R, _: ()) -> io::Result<Self> {
        let version = reader.read_u64::<LittleEndian>()?;

        let _account = if version <= 5 {
            reader.read_u64::<LittleEndian>()?
        } else {
            0
        };
        let fvk = <T::Fvk as ReadableWriteable<()>>::read(&mut reader, ())?;

        let mut diversifier_bytes = [0u8; 11];
        reader.read_exact(&mut diversifier_bytes)?;
        let diversifier = T::Diversifier::from_bytes(diversifier_bytes);

        let note =
            <T::Note as ReadableWriteable<_>>::read(&mut reader, (fvk.clone(), diversifier))?;

        let witnesses_vec = Vector::read(&mut reader, |r| IncrementalWitness::<T::Node>::read(r))?;
        let top_height = if version < 20 {
            0
        } else {
            reader.read_u64::<LittleEndian>()?
        };
        let witnesses = WitnessCache::new(witnesses_vec, top_height);

        let mut nullifier = [0u8; 32];
        reader.read_exact(&mut nullifier)?;
        let nullifier = T::Nullifier::from_bytes(nullifier);

        // Note that this is only the spent field, we ignore the unconfirmed_spent field.
        // The reason is that unconfirmed spents are only in memory, and we need to get the actual value of spent
        // from the blockchain anyway.
        let spent = if version <= 5 {
            let spent = Optional::read(&mut reader, |r| {
                let mut transaction_id_bytes = [0u8; 32];
                r.read_exact(&mut transaction_id_bytes)?;
                Ok(TxId::from_bytes(transaction_id_bytes))
            })?;

            let spent_at_height = if version >= 2 {
                Optional::read(&mut reader, |r| r.read_i32::<LittleEndian>())?
            } else {
                None
            };

            if spent.is_some() && spent_at_height.is_some() {
                Some((spent.unwrap(), spent_at_height.unwrap() as u32))
            } else {
                None
            }
        } else {
            Optional::read(&mut reader, |r| {
                let mut transaction_id_bytes = [0u8; 32];
                r.read_exact(&mut transaction_id_bytes)?;
                let height = r.read_u32::<LittleEndian>()?;
                Ok((TxId::from_bytes(transaction_id_bytes), height))
            })?
        };

        let unconfirmed_spent = if version <= 4 {
            None
        } else {
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

        let have_spending_key = if version <= 2 {
            true // Will get populated in the lightwallet::read() method, for now assume true
        } else {
            reader.read_u8()? > 0
        };

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
        writer.write_u64::<LittleEndian>(Self::VERSION)?;

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

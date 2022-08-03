//! Provides unifying interfaces for transaction management across Sapling and Orchard
use super::{
    data::{
        OrchardNoteAndMetadata, SaplingNoteAndMetadata, WalletNullifier, WalletTx, WitnessCache,
    },
    keys::{orchard::OrchardKey, sapling::SaplingKey, Keys},
    transactions::WalletTxns,
};
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
use zcash_address::unified::{self, Encoding as _, Receiver};
use zcash_client_backend::encoding::encode_payment_address;
use zcash_note_encryption::{Domain, ShieldedOutput, ENC_CIPHERTEXT_SIZE};
use zcash_primitives::{
    consensus::{BlockHeight, Parameters},
    keys::OutgoingViewingKey as SaplingOutgoingViewingKey,
    memo::{Memo, MemoBytes},
    merkle_tree::Hashable,
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

/// As with `FromBytes`, provision of a generic interface
pub(crate) trait FromCommitment {
    fn from_commitment(from: &[u8; 32]) -> Self;
}

impl FromCommitment for SaplingNode {
    fn from_commitment(from: &[u8; 32]) -> Self {
        Self::new(*from)
    }
}
impl FromCommitment for MerkleHashOrchard {
    fn from_commitment(from: &[u8; 32]) -> Self {
        Self::from_bytes(from).unwrap()
    }
}

/// The component that transfers value.  In the common case, from one output to another.
pub(crate) trait Spend {
    type Nullifier: PartialEq + FromBytes<32> + UnspentFromWalletTxns;
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
    fn b32encode_for_network(&self, chain: Network) -> String;
}

impl Recipient for OrchardAddress {
    type Diversifier = OrchardDiversifier;

    fn diversifier(&self) -> Self::Diversifier {
        OrchardAddress::diversifier(&self)
    }

    fn b32encode_for_network(&self, chain: Network) -> String {
        unified::Address::try_from_items(vec![Receiver::Orchard(self.to_raw_address_bytes())])
            .unwrap()
            .encode(&chain.address_network().unwrap())
    }
}

impl Recipient for SaplingAddress {
    type Diversifier = SaplingDiversifier;

    fn diversifier(&self) -> Self::Diversifier {
        *SaplingAddress::diversifier(&self)
    }

    fn b32encode_for_network(&self, chain: Network) -> String {
        encode_payment_address(chain.hrp_sapling_payment_address(), self)
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
pub(crate) trait UnspentFromWalletTxns
where
    Self: Sized,
{
    fn get_nullifiers_of_unspent_notes_from_wallet(
        transactions: &WalletTxns,
    ) -> Vec<(Self, u64, TxId)>;
}

impl UnspentFromWalletTxns for SaplingNullifier {
    fn get_nullifiers_of_unspent_notes_from_wallet(
        transactions: &WalletTxns,
    ) -> Vec<(Self, u64, TxId)> {
        transactions.get_nullifiers_of_unspent_sapling_notes()
    }
}

impl UnspentFromWalletTxns for OrchardNullifier {
    fn get_nullifiers_of_unspent_notes_from_wallet(
        transactions: &WalletTxns,
    ) -> Vec<(Self, u64, TxId)> {
        transactions.get_nullifiers_of_unspent_orchard_notes()
    }
}

///  Complete note contents, and additional metadata.
pub(crate) trait NoteAndMetadata: Sized {
    type Fvk: Clone;
    type Diversifier;
    type Note: PartialEq;
    type Node: Hashable;
    type Nullifier: PartialEq + ToBytes<32> + FromBytes<32> + UnspentFromWalletTxns;
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
    fn memo_mut(&mut self) -> &mut Option<Memo>;
    fn note(&self) -> &Self::Note;
    fn nullifier(&self) -> Self::Nullifier;
    fn value(note: &Self::Note) -> u64;
    fn witnesses(&mut self) -> &mut WitnessCache<Self::Node>;
    fn wallet_transaction_notes_mut(wallet_transaction: &mut WalletTx) -> &mut Vec<Self>;
}

impl NoteAndMetadata for SaplingNoteAndMetadata {
    type Fvk = SaplingExtendedFullViewingKey;
    type Diversifier = SaplingDiversifier;
    type Note = SaplingNote;
    type Node = SaplingNode;
    type Nullifier = SaplingNullifier;

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

    fn wallet_transaction_notes_mut(wallet_transaction: &mut WalletTx) -> &mut Vec<Self> {
        &mut wallet_transaction.sapling_notes
    }

    fn witnesses(&mut self) -> &mut WitnessCache<Self::Node> {
        &mut self.witnesses
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

    type Address = zcash_address::unified::Address;

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

impl NoteAndMetadata for OrchardNoteAndMetadata {
    type Fvk = OrchardFullViewingKey;
    type Diversifier = OrchardDiversifier;
    type Note = OrchardNote;
    type Node = MerkleHashOrchard;
    type Nullifier = OrchardNullifier;

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

    fn wallet_transaction_notes_mut(wallet_transaction: &mut WalletTx) -> &mut Vec<Self> {
        &mut wallet_transaction.orchard_notes
    }

    fn witnesses(&mut self) -> &mut WitnessCache<Self::Node> {
        &mut self.witnesses
    }
}

/// An interface that provides a generic mechanism for the generation of a mutable vector of Notes
/// a `WalletNote` is a rich data structure that implements the NoteData trait which provides all
/// of the information in the note, as well as note metadata.
pub(crate) trait DomainWalletExt<P: Parameters>: Domain
where
    Self: Sized,
    Self::Note: PartialEq,
    Self::Recipient: Recipient,
    Self::Note: PartialEq,
{
    type Fvk: Clone;
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

    fn wallet_notes_mut(_: &mut WalletTx) -> &mut Vec<Self::WalletNote>;
}

impl<P: Parameters> DomainWalletExt<P> for SaplingDomain<P> {
    type Fvk = SaplingExtendedFullViewingKey;

    type WalletNote = SaplingNoteAndMetadata;

    type Key = SaplingKey;

    type Bundle = SaplingBundle<SaplingAuthorized>;

    fn wallet_notes_mut(transaction: &mut WalletTx) -> &mut Vec<Self::WalletNote> {
        &mut transaction.sapling_notes
    }
}

impl<P: Parameters> DomainWalletExt<P> for OrchardDomain {
    type Fvk = OrchardFullViewingKey;

    type WalletNote = OrchardNoteAndMetadata;

    type Key = OrchardKey;

    type Bundle = OrchardBundle<OrchardAuthorized, Amount>;

    fn wallet_notes_mut(transaction: &mut WalletTx) -> &mut Vec<Self::WalletNote> {
        &mut transaction.orchard_notes
    }
}

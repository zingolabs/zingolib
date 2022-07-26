use super::{
    data::{OrchardNoteData, SaplingNoteData, WalletNullifier, WalletTx, WitnessCache},
    keys::{orchard::OrchardKey, sapling::SaplingKey},
};
use nonempty::NonEmpty;
use orchard::{
    bundle::{Authorization as OrchardAuthorization, Bundle as OrchardBundle},
    keys::{
        Diversifier as OrchardDiversifier, FullViewingKey as OrchardFullViewingKey,
        IncomingViewingKey as OrchardIncomingViewingKey,
        OutgoingViewingKey as OrchardOutgoingViewingKey, SpendingKey as OrchardSpendingKey,
    },
    note::{Note as OrchardNote, Nullifier as OrchardNullifier},
    note_encryption::OrchardDomain,
    tree::MerkleHashOrchard,
    Action, Address as OrchardAddress,
};
use zcash_address::unified::{self, Encoding as _, Receiver};
use zcash_client_backend::encoding::encode_payment_address;
use zcash_note_encryption::{Domain, ShieldedOutput, ENC_CIPHERTEXT_SIZE};
use zcash_primitives::{
    consensus::Parameters,
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
                Authorization as SaplingAuthorization, Bundle as SaplingBundle, GrothProofBytes,
            },
            OutputDescription, SpendDescription,
        },
        TxId,
    },
    zip32::{
        ExtendedFullViewingKey as SaplingExtendedFullViewingKey,
        ExtendedSpendingKey as SaplingExtendedSpendingKey,
    },
};
use zingoconfig::Network;

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

pub(crate) trait ShieldedOutputExt<D: Domain>:
    ShieldedOutput<D, ENC_CIPHERTEXT_SIZE>
{
    fn cv(&self) -> D::ValueCommitment;
    fn out_ciphertext(&self) -> [u8; 80];
}

impl<A> ShieldedOutputExt<OrchardDomain> for Action<A> {
    fn cv(&self) -> orchard::value::ValueCommitment {
        self.cv_net().clone()
    }

    fn out_ciphertext(&self) -> [u8; 80] {
        self.encrypted_note().out_ciphertext
    }
}

impl ShieldedOutputExt<SaplingDomain<Network>> for OutputDescription<GrothProofBytes> {
    fn cv(&self) -> <SaplingDomain<Network> as Domain>::ValueCommitment {
        self.cv
    }

    fn out_ciphertext(&self) -> [u8; 80] {
        self.out_ciphertext
    }
}

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

pub(crate) trait Spend {
    type Nullifier;
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

pub(crate) trait Recipient {
    type Diversifier;
    fn diversifier(&self) -> Self::Diversifier;
    fn encode(&self, chain: Network) -> String;
}

impl Recipient for OrchardAddress {
    type Diversifier = OrchardDiversifier;

    fn diversifier(&self) -> Self::Diversifier {
        OrchardAddress::diversifier(&self)
    }

    fn encode(&self, chain: Network) -> String {
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

    fn encode(&self, chain: Network) -> String {
        encode_payment_address(chain.hrp_sapling_payment_address(), self)
    }
}

pub(crate) trait Bundle {
    type Spend;
    type Output;
    type Spends: IntoIterator<Item = Self::Spend>;
    type Outputs: IntoIterator<Item = Self::Output>;
    fn spends(&self) -> &Self::Spends;
    fn outputs(&self) -> &Self::Outputs;
}

impl<A: SaplingAuthorization> Bundle for SaplingBundle<A> {
    type Spend = SpendDescription<A>;
    type Output = OutputDescription<A::Proof>;
    type Spends = Vec<Self::Spend>;
    type Outputs = Vec<Self::Output>;
    fn spends(&self) -> &Self::Spends {
        &self.shielded_spends
    }

    fn outputs(&self) -> &Self::Outputs {
        &self.shielded_outputs
    }
}

impl<A: OrchardAuthorization, V> Bundle for OrchardBundle<A, V> {
    type Spend = Action<A::SpendAuth>;
    type Output = Action<A::SpendAuth>;
    type Spends = NonEmpty<Self::Spend>;
    type Outputs = NonEmpty<Self::Output>;

    fn spends(&self) -> &Self::Spends {
        self.actions()
    }

    fn outputs(&self) -> &Self::Outputs {
        self.actions()
    }
}

pub(crate) trait NoteData: Sized {
    type Fvk: Clone;
    type Diversifier;
    type Note: PartialEq;
    type Node: Hashable;
    type Nullifier: PartialEq + ToBytes<32> + FromBytes<32>;
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

impl NoteData for SaplingNoteData {
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

pub(crate) trait WalletKey {
    type Fvk;
    type Ivk;
    type Ovk;
    type Sk;
    fn fvk(&self) -> Option<Self::Fvk>;
    fn ivk(&self) -> Option<Self::Ivk>;
    fn ovk(&self) -> Option<Self::Ovk>;
    fn sk(&self) -> Option<Self::Sk>;
}

impl WalletKey for SaplingKey {
    type Fvk = SaplingExtendedFullViewingKey;

    type Ivk = SaplingIvk;

    type Ovk = SaplingOutgoingViewingKey;

    type Sk = SaplingExtendedSpendingKey;

    fn fvk(&self) -> Option<Self::Fvk> {
        Some(self.extfvk.clone())
    }

    fn ivk(&self) -> Option<Self::Ivk> {
        Some(self.extfvk.fvk.vk.ivk())
    }

    fn ovk(&self) -> Option<Self::Ovk> {
        Some(self.extfvk.fvk.ovk)
    }

    fn sk(&self) -> Option<Self::Sk> {
        self.extsk.clone()
    }
}

impl WalletKey for OrchardKey {
    type Fvk = OrchardFullViewingKey;

    type Ivk = OrchardIncomingViewingKey;

    type Ovk = OrchardOutgoingViewingKey;

    type Sk = OrchardSpendingKey;

    fn fvk(&self) -> Option<Self::Fvk> {
        (&self.key).try_into().ok()
    }

    fn ivk(&self) -> Option<Self::Ivk> {
        (&self.key).try_into().ok()
    }

    fn ovk(&self) -> Option<Self::Ovk> {
        (&self.key).try_into().ok()
    }

    fn sk(&self) -> Option<Self::Sk> {
        (&self.key).try_into().ok()
    }
}

impl NoteData for OrchardNoteData {
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

pub(crate) trait DomainWalletExt: Domain {
    type Fvk: Clone;
    type WalletNote: NoteData;

    fn wallet_notes_mut(_: &mut WalletTx) -> &mut Vec<Self::WalletNote>;
}

impl<P: Parameters> DomainWalletExt for SaplingDomain<P> {
    type Fvk = SaplingExtendedFullViewingKey;

    type WalletNote = SaplingNoteData;

    fn wallet_notes_mut(transaction: &mut WalletTx) -> &mut Vec<Self::WalletNote> {
        &mut transaction.sapling_notes
    }
}

impl DomainWalletExt for OrchardDomain {
    type Fvk = OrchardFullViewingKey;

    type WalletNote = OrchardNoteData;

    fn wallet_notes_mut(transaction: &mut WalletTx) -> &mut Vec<Self::WalletNote> {
        &mut transaction.orchard_notes
    }
}

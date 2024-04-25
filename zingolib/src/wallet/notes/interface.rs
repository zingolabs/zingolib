//! TODO: Add Mod Description Here!
use incrementalmerkletree::{Hashable, Position};
use zcash_client_backend::{PoolType, ShieldedProtocol};
use zcash_primitives::{memo::Memo, merkle_tree::HashSer, transaction::TxId};

use super::{
    super::{
        data::TransactionRecord,
        keys::unified::WalletCapability,
        traits::{FromBytes, FromCommitment, Nullifier, ReadableWriteable, ToBytes},
        Pool,
    },
    query::{OutputPoolQuery, OutputQuery, OutputSpendStatusQuery},
};

/// TODO: Add Doc Comment Here!
pub trait OutputInterface: Sized {
    /// returns the zcash_client_backend PoolType enum (one of 3)
    fn pool_type(&self) -> PoolType;

    /// number of Zatoshis unlocked by the note
    fn value(&self) -> u64;

    /// TODO: Add Doc Comment Here!
    fn spent(&self) -> &Option<(TxId, u32)>;

    /// TODO: Add Doc Comment Here!
    fn spent_mut(&mut self) -> &mut Option<(TxId, u32)>;

    /// TODO: Add Doc Comment Here!
    fn pending_spent(&self) -> &Option<(TxId, u32)>;

    /// TODO: Add Doc Comment Here!
    fn pending_spent_mut(&mut self) -> &mut Option<(TxId, u32)>;

    /// Returns true if the note has one of the spend statuses enumerated by the query
    fn spend_status_query(&self, query: OutputSpendStatusQuery) -> bool {
        (*query.unspent() && !self.is_spent() && !self.is_pending_spent())
            || (*query.pending_spent() && self.is_pending_spent())
            || (*query.spent() && self.is_spent())
    }

    /// Returns true if the note is unspent (spendable).
    fn is_unspent(&self) -> bool {
        self.spend_status_query(OutputSpendStatusQuery::new(true, false, false))
    }

    /// Returns true if the note has been presumptively spent but the spent has not been validated.
    fn is_pending_spent(&self) -> bool {
        self.spend_status_query(OutputSpendStatusQuery::new(false, true, false))
    }

    /// returns true if the note is confirmed spent
    fn is_spent(&self) -> bool {
        self.spend_status_query(OutputSpendStatusQuery::new(false, false, true))
    }

    /// Returns true if the note is one of the pools enumerated by the query.
    fn pool_query(&self, query: OutputPoolQuery) -> bool {
        (*query.transparent() && self.pool_type() == PoolType::Transparent)
            || (*query.sapling()
                && self.pool_type() == PoolType::Shielded(ShieldedProtocol::Sapling))
            || (*query.orchard()
                && self.pool_type() == PoolType::Shielded(ShieldedProtocol::Orchard))
    }

    /// Returns true if the note is one of the spend statuses enumerated by the query AND one of the pools enumerated by the query.
    fn query(&self, query: OutputQuery) -> bool {
        self.spend_status_query(*query.spend_status()) && self.pool_query(*query.pools())
    }

    /// Returns a vec of the Outputs in the TransactionRecord that fit the OutputSpendStatusQuery in this pool.
    fn transaction_record_to_outputs_vec(
        transaction_record: &mut TransactionRecord,
        spend_status_query: OutputSpendStatusQuery,
    ) -> Vec<&mut Self>;
}

///   ShieldedNotes are either part of a Sapling or Orchard Pool
pub trait ShieldedNoteInterface: OutputInterface + Sized {
    /// TODO: Add Doc Comment Here!
    type Diversifier: Copy + FromBytes<11> + ToBytes<11>;
    /// TODO: Add Doc Comment Here!
    type Note: PartialEq
        + for<'a> ReadableWriteable<(Self::Diversifier, &'a WalletCapability)>
        + Clone;
    /// TODO: Add Doc Comment Here!
    type Node: Hashable + HashSer + FromCommitment + Send + Clone + PartialEq + Eq;
    /// TODO: Add Doc Comment Here!
    type Nullifier: Nullifier;

    /// TODO: Add Doc Comment Here!
    fn diversifier(&self) -> &Self::Diversifier;

    /// TODO: Add Doc Comment Here!
    #[allow(clippy::too_many_arguments)]
    fn from_parts(
        diversifier: Self::Diversifier,
        note: Self::Note,
        position_of_commitment_to_witness: Option<Position>,
        nullifier: Option<Self::Nullifier>,
        spent: Option<(TxId, u32)>,
        unconfirmed_spent: Option<(TxId, u32)>,
        memo: Option<Memo>,
        is_change: bool,
        have_spending_key: bool,
        output_index: Option<u32>,
    ) -> Self;

    /// TODO: Add Doc Comment Here!
    fn get_deprecated_serialized_view_key_buffer() -> Vec<u8>;

    /// TODO: Add Doc Comment Here!
    fn have_spending_key(&self) -> bool;

    /// TODO: Add Doc Comment Here!
    fn is_change(&self) -> bool;

    /// TODO: Add Doc Comment Here!
    fn is_change_mut(&mut self) -> &mut bool;

    /// TODO: Add Doc Comment Here!
    fn memo(&self) -> &Option<Memo>;

    /// TODO: Add Doc Comment Here!
    fn memo_mut(&mut self) -> &mut Option<Memo>;

    /// TODO: Add Doc Comment Here!
    fn note(&self) -> &Self::Note;

    /// TODO: Add Doc Comment Here!
    fn nullifier(&self) -> Option<Self::Nullifier>;

    /// TODO: Add Doc Comment Here!
    fn nullifier_mut(&mut self) -> &mut Option<Self::Nullifier>;

    /// TODO: Add Doc Comment Here!
    fn output_index(&self) -> &Option<u32>;

    /// TODO: Add Doc Comment Here!
    fn pending_receipt(&self) -> bool {
        self.nullifier().is_none()
    }

    /// TODO: Add Doc Comment Here!
    fn pool() -> Pool;

    /// TODO: Add Doc Comment Here!
    fn transaction_metadata_notes(wallet_transaction: &TransactionRecord) -> &Vec<Self>;

    /// TODO: Add Doc Comment Here!
    fn transaction_metadata_notes_mut(wallet_transaction: &mut TransactionRecord)
        -> &mut Vec<Self>;

    /// TODO: Add Doc Comment Here!
    fn value_from_note(note: &Self::Note) -> u64;

    /// TODO: Add Doc Comment Here!
    fn witnessed_position(&self) -> &Option<Position>;

    /// TODO: Add Doc Comment Here!
    fn witnessed_position_mut(&mut self) -> &mut Option<Position>;

    /// TODO: Add Doc Comment Here!
    fn to_zcb_note(&self) -> zcash_client_backend::wallet::Note;
}

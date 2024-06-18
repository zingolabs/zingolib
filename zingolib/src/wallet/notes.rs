//! All things needed to create, manaage, and use notes
pub mod interface;
pub use interface::OutputInterface;
pub use interface::ShieldedNoteInterface;
pub mod transparent;
pub use transparent::TransparentOutput;
pub mod sapling;
pub use sapling::SaplingNote;
pub mod orchard;
pub use orchard::OrchardNote;
pub mod query;

use zcash_client_backend::PoolType;

use zcash_primitives::transaction::TxId;

use crate::wallet::notes::query::OutputPoolQuery;
use crate::wallet::notes::query::OutputQuery;
use crate::wallet::notes::query::OutputSpendStatusQuery;

/// An interface for accessing all the common functionality of all the outputs
#[enum_dispatch::enum_dispatch(OutputInterface)]
#[non_exhaustive] // We can add new pools later
pub enum AnyPoolOutput {
    /// Transparent Outputs
    TransparentOutput,
    /// Sapling Notes
    SaplingNote,
    /// Orchard Notes
    OrchardNote,
}
impl AnyPoolOutput {
    /// All the output records
    pub fn get_record_outputs(
        transaction_record: &super::transaction_record::TransactionRecord,
    ) -> Vec<Self> {
        transaction_record
            .transparent_outputs
            .iter()
            .map(|output| Self::TransparentOutput(output.clone()))
            .chain(
                transaction_record
                    .sapling_notes
                    .iter()
                    .map(|output| Self::SaplingNote(output.clone())),
            )
            .chain(
                transaction_record
                    .orchard_notes
                    .iter()
                    .map(|output| Self::OrchardNote(output.clone())),
            )
            .collect()
    }

    /// Every notes' outputinterface for a given spend status
    pub fn get_all_outputs_with_status(
        transaction_record: &super::transaction_record::TransactionRecord,
        spend_status_query: OutputSpendStatusQuery,
    ) -> Vec<Self> {
        transaction_record
            .transparent_outputs
            .iter()
            .filter(|output| output.spend_status_query(spend_status_query))
            .map(|output| Self::TransparentOutput(output.clone()))
            .chain(
                transaction_record
                    .sapling_notes
                    .iter()
                    .filter(|output| output.spend_status_query(spend_status_query))
                    .map(|output| Self::SaplingNote(output.clone())),
            )
            .chain(
                transaction_record
                    .orchard_notes
                    .iter()
                    .filter(|output| output.spend_status_query(spend_status_query))
                    .map(|output| Self::OrchardNote(output.clone())),
            )
            .collect()
    }
}
/// This triple of values uniquely over-identifies a value transfer on a zcash blockchain.
/// "Over" because pool is not necessary for a unique ID.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct OutputId {
    /// A unique ID of the transaction.  For v5 protocol and later, to enforce
    /// non-malleability it is derived exclusively from effecting data
    /// (data that is necessary for transaction validity.
    pub txid: TxId,
    /// Shielded (two kinds) or Transparent
    pub pool: PoolType,
    /// The unique position of this value transfer in the transaction.
    pub index: u32,
}
impl OutputId {
    /// The primary constructor, note index means FLARRGGGLLLE!
    pub fn from_parts(txid: TxId, pool: PoolType, index: u32) -> Self {
        OutputId { txid, pool, index }
    }
}

impl std::fmt::Display for OutputId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "txid {}, {:?}, index {}",
            self.txid, self.pool, self.index,
        )
    }
}

#[cfg(test)]
pub mod mocks {
    //! Mock version of the struct for testing
    use zcash_client_backend::{wallet::NoteId, ShieldedProtocol};
    use zcash_primitives::transaction::TxId;

    use crate::mocks::{build_method, default_txid};

    /// to build a mock NoteRecordIdentifier
    pub struct NoteIdBuilder {
        txid: Option<TxId>,
        shpool: Option<ShieldedProtocol>,
        index: Option<u16>,
    }
    impl NoteIdBuilder {
        /// blank builder
        pub fn new() -> Self {
            Self {
                txid: None,
                shpool: None,
                index: None,
            }
        }
        // Methods to set each field
        build_method!(txid, TxId);
        build_method!(shpool, ShieldedProtocol);
        build_method!(index, u16);

        /// selects a random probablistically unique txid
        pub fn randomize_txid(&mut self) -> &mut Self {
            self.txid(crate::mocks::random_txid())
        }

        /// builds a mock NoteRecordIdentifier after all pieces are supplied
        pub fn build(self) -> NoteId {
            NoteId::new(
                self.txid.unwrap(),
                self.shpool.unwrap(),
                self.index.unwrap(),
            )
        }
    }

    impl Default for NoteIdBuilder {
        fn default() -> Self {
            let mut builder = Self::new();
            builder
                .txid(default_txid())
                .shpool(zcash_client_backend::ShieldedProtocol::Orchard)
                .index(0);
            builder
        }
    }
}

#[cfg(test)]
pub mod tests {
    use zcash_client_backend::PoolType;

    use crate::{
        mocks::default_txid,
        wallet::notes::{
            query::OutputQuery, sapling::mocks::SaplingNoteBuilder,
            transparent::mocks::TransparentOutputBuilder, OutputInterface,
        },
    };

    use super::query::{OutputPoolQuery, OutputSpendStatusQuery};

    #[test]
    fn note_queries() {
        let spend = Some((default_txid(), 112358));

        let transparent_unspent_note = TransparentOutputBuilder::default().build();
        let transparent_pending_spent_note = TransparentOutputBuilder::default()
            .pending_spent(spend)
            .clone()
            .build();
        let transparent_spent_note = TransparentOutputBuilder::default()
            .spent(spend)
            .clone()
            .build();
        let sapling_unspent_note = SaplingNoteBuilder::default().build();
        let sapling_pending_spent_note = SaplingNoteBuilder::default()
            .pending_spent(spend)
            .clone()
            .build();
        let sapling_spent_note = SaplingNoteBuilder::default().spent(spend).clone().build();

        let unspent_query = OutputSpendStatusQuery::only_unspent();
        let pending_or_spent_query = OutputSpendStatusQuery::spentish();
        let spent_query = OutputSpendStatusQuery::only_spent();

        let transparent_query = OutputPoolQuery::one_pool(PoolType::Transparent);
        let shielded_query = OutputPoolQuery::shielded();
        let any_pool_query = OutputPoolQuery::any();

        let unspent_transparent_query = OutputQuery {
            spend_status: unspent_query,
            pools: transparent_query,
        };
        let unspent_any_pool_query = OutputQuery {
            spend_status: unspent_query,
            pools: any_pool_query,
        };
        let pending_or_spent_transparent_query = OutputQuery {
            spend_status: pending_or_spent_query,
            pools: transparent_query,
        };
        let pending_or_spent_shielded_query = OutputQuery {
            spend_status: pending_or_spent_query,
            pools: shielded_query,
        };
        let spent_shielded_query = OutputQuery {
            spend_status: spent_query,
            pools: shielded_query,
        };
        let spent_any_pool_query = OutputQuery {
            spend_status: spent_query,
            pools: any_pool_query,
        };

        assert!(transparent_unspent_note.query(unspent_transparent_query));
        assert!(transparent_unspent_note.query(unspent_any_pool_query));
        assert!(!transparent_unspent_note.query(pending_or_spent_transparent_query));
        assert!(!transparent_unspent_note.query(pending_or_spent_shielded_query));
        assert!(!transparent_unspent_note.query(spent_shielded_query));
        assert!(!transparent_unspent_note.query(spent_any_pool_query));

        assert!(!transparent_pending_spent_note.query(unspent_transparent_query));
        assert!(!transparent_pending_spent_note.query(unspent_any_pool_query));
        assert!(transparent_pending_spent_note.query(pending_or_spent_transparent_query));
        assert!(!transparent_pending_spent_note.query(pending_or_spent_shielded_query));
        assert!(!transparent_pending_spent_note.query(spent_shielded_query));
        assert!(!transparent_pending_spent_note.query(spent_any_pool_query));

        assert!(!transparent_spent_note.query(unspent_transparent_query));
        assert!(!transparent_spent_note.query(unspent_any_pool_query));
        assert!(transparent_spent_note.query(pending_or_spent_transparent_query));
        assert!(!transparent_spent_note.query(pending_or_spent_shielded_query));
        assert!(!transparent_spent_note.query(spent_shielded_query));
        assert!(transparent_spent_note.query(spent_any_pool_query));

        assert!(!sapling_unspent_note.query(unspent_transparent_query));
        assert!(sapling_unspent_note.query(unspent_any_pool_query));
        assert!(!sapling_unspent_note.query(pending_or_spent_transparent_query));
        assert!(!sapling_unspent_note.query(pending_or_spent_shielded_query));
        assert!(!sapling_unspent_note.query(spent_shielded_query));
        assert!(!sapling_unspent_note.query(spent_any_pool_query));

        assert!(!sapling_pending_spent_note.query(unspent_transparent_query));
        assert!(!sapling_pending_spent_note.query(unspent_any_pool_query));
        assert!(!sapling_pending_spent_note.query(pending_or_spent_transparent_query));
        assert!(sapling_pending_spent_note.query(pending_or_spent_shielded_query));
        assert!(!sapling_pending_spent_note.query(spent_shielded_query));
        assert!(!sapling_pending_spent_note.query(spent_any_pool_query));

        assert!(!sapling_spent_note.query(unspent_transparent_query));
        assert!(!sapling_spent_note.query(unspent_any_pool_query));
        assert!(!sapling_spent_note.query(pending_or_spent_transparent_query));
        assert!(sapling_spent_note.query(pending_or_spent_shielded_query));
        assert!(sapling_spent_note.query(spent_shielded_query));
        assert!(sapling_spent_note.query(spent_any_pool_query));
    }
}

//! All things needed to create, manaage, and use notes
pub mod interface;
pub use interface::NoteInterface;
pub use interface::ShieldedNoteInterface;
pub mod transparent;
pub use transparent::TransparentNote;
pub mod sapling;
pub use sapling::SaplingNote;
pub mod orchard;
pub use orchard::OrchardNote;
pub mod query;

use zcash_client_backend::PoolType;
use zcash_primitives::transaction::TxId;

/// This triple of values uniquely identifies an entry on a zcash blockchain.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NoteRecordIdentifier {
    /// TODO: Add Doc Comment Here!
    pub txid: TxId,
    /// TODO: Add Doc Comment Here!
    pub pool: PoolType,
    /// TODO: Add Doc Comment Here!
    pub index: u32,
}

impl NoteRecordIdentifier {
    /// The primary constructor, note index means FLARRGGGLLLE!
    pub fn from_parts(txid: TxId, pool: PoolType, index: u32) -> Self {
        NoteRecordIdentifier { txid, pool, index }
    }
}

impl std::fmt::Display for NoteRecordIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "txid {}, {:?}, index {}",
            self.txid, self.pool, self.index,
        )
    }
}

#[cfg(any(test, feature = "test-features"))]
pub mod mocks {
    //! Mock version of the struct for testing
    use zcash_client_backend::PoolType;
    use zcash_primitives::transaction::TxId;

    use crate::test_framework::mocks::{build_method, default_txid};

    use super::NoteRecordIdentifier;

    /// to build a mock NoteRecordIdentifier
    pub struct NoteRecordIdentifierBuilder {
        txid: Option<TxId>,
        pool: Option<PoolType>,
        index: Option<u32>,
    }
    #[allow(dead_code)] //TODO:  fix this gross hack that I tossed in to silence the language-analyzer false positive
    impl NoteRecordIdentifierBuilder {
        /// blank builder
        pub fn new() -> Self {
            Self {
                txid: None,
                pool: None,
                index: None,
            }
        }
        // Methods to set each field
        build_method!(txid, TxId);
        build_method!(pool, PoolType);
        build_method!(index, u32);

        /// selects a random probablistically unique txid
        pub fn randomize_txid(self) -> Self {
            self.txid(crate::test_framework::mocks::random_txid())
        }

        /// builds a mock NoteRecordIdentifier after all pieces are supplied
        pub fn build(self) -> NoteRecordIdentifier {
            NoteRecordIdentifier::from_parts(
                self.txid.unwrap(),
                self.pool.unwrap(),
                self.index.unwrap(),
            )
        }
    }

    impl Default for NoteRecordIdentifierBuilder {
        fn default() -> Self {
            Self::new()
                .txid(default_txid())
                .pool(PoolType::Shielded(
                    zcash_client_backend::ShieldedProtocol::Orchard,
                ))
                .index(0)
        }
    }
}

#[cfg(test)]
pub mod tests {
    use crate::{
        test_framework::mocks::default_txid,
        wallet::notes::{
            query::NoteQuery, sapling::mocks::SaplingNoteBuilder,
            transparent::mocks::TransparentNoteBuilder, NoteInterface,
        },
    };

    use super::query::{NotePoolQuery, NoteSpendStatusQuery};

    #[test]
    fn note_queries() {
        let spend = Some((default_txid(), 112358));

        let transparent_unspent_note = TransparentNoteBuilder::default().build();
        let transparent_pending_spent_note = TransparentNoteBuilder::default()
            .unconfirmed_spent(spend)
            .build();
        let transparent_spent_note = TransparentNoteBuilder::default().spent(spend).build();
        let sapling_unspent_note = SaplingNoteBuilder::default().build();
        let sapling_pending_spent_note = SaplingNoteBuilder::default()
            .unconfirmed_spent(spend)
            .build();
        let sapling_spent_note = SaplingNoteBuilder::default().spent(spend).build();

        let unspent_query = NoteSpendStatusQuery::new(true, false, false);
        let pending_or_spent_query = NoteSpendStatusQuery::new(false, true, true);
        let spent_query = NoteSpendStatusQuery::new(false, false, true);

        let transparent_query = NotePoolQuery::new(true, false, false);
        let shielded_query = NotePoolQuery::new(false, true, true);
        let any_pool_query = NotePoolQuery::new(true, true, true);

        let unspent_transparent_query = NoteQuery::new(unspent_query, transparent_query);
        let unspent_any_pool_query = NoteQuery::new(unspent_query, any_pool_query);
        let pending_or_spent_transparent_query =
            NoteQuery::new(pending_or_spent_query, transparent_query);
        let pending_or_spent_shielded_query =
            NoteQuery::new(pending_or_spent_query, shielded_query);
        let spent_shielded_query = NoteQuery::new(spent_query, shielded_query);
        let spent_any_pool_query = NoteQuery::new(spent_query, any_pool_query);

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

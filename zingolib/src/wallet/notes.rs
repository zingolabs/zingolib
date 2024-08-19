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
use zingo_status::confirmation_status::ConfirmationStatus;

/// An interface for accessing all the common functionality of all the outputs
#[enum_dispatch::enum_dispatch(OutputInterface)]
#[non_exhaustive] // We can add new pools later
#[derive(Clone, Debug)]
pub enum Output {
    /// Transparent Outputs
    TransparentOutput,
    /// Sapling Notes
    SaplingNote,
    /// Orchard Notes
    OrchardNote,
}
impl Output {
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

    /// this sums the value of a vec of outputs, ignoring marginal fees
    pub fn sum_gross_value(list: Vec<Self>) -> u64 {
        list.iter().fold(0, |total, output| total + output.value())
    }
}

#[cfg(test)]
pub mod mocks {
    //! Mock version of the struct for testing
    use zcash_client_backend::{wallet::NoteId, ShieldedProtocol};
    use zcash_primitives::transaction::TxId;

    use crate::{mocks::default_txid, utils::build_method};

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

    use zingo_status::confirmation_status::ConfirmationStatus::Confirmed;
    use zingo_status::confirmation_status::ConfirmationStatus::Mempool;

    #[test]
    fn note_queries() {
        let confirmed_spend = Some((default_txid(), Confirmed(112358.into())));
        let pending_spend = Some((default_txid(), Mempool(112357.into())));

        let transparent_unspent_note = TransparentOutputBuilder::default().build();
        let transparent_pending_spent_note = TransparentOutputBuilder::default()
            .spending_tx_status(pending_spend)
            .clone()
            .build();
        let transparent_spent_note = TransparentOutputBuilder::default()
            .spending_tx_status(confirmed_spend)
            .clone()
            .build();
        let sapling_unspent_note = SaplingNoteBuilder::default().build();
        let sapling_pending_spent_note = SaplingNoteBuilder::default()
            .spending_tx_status(pending_spend)
            .clone()
            .build();
        let sapling_spent_note = SaplingNoteBuilder::default()
            .spending_tx_status(confirmed_spend)
            .clone()
            .build();

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

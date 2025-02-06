//! All things needed to create, manaage, and use notes
pub mod interface;
pub use interface::OutputInterface as OldOutputInterface;
pub use interface::ShieldedNoteInterface;
pub mod transparent;
pub use transparent::TransparentOutput;
pub mod sapling;
pub use sapling::SaplingNote;
pub mod orchard;
pub use orchard::OrchardNote;
pub mod query;

use zcash_primitives::transaction::TxId;
use zingo_sync::primitives::OutputInterface;
use zingo_sync::primitives::WalletTransaction;

use crate::wallet::notes::query::OutputQuery;
use crate::wallet::notes::query::OutputSpendStatusQuery;
use zingo_status::confirmation_status::ConfirmationStatus;

use super::LightWallet;

/// Spend status of an output
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SpendStatus {
    /// Output is not spent.
    Unspent,
    /// Output is pending spent.
    /// The transaction consuming this output has been calculated.
    CalculatedSpent(TxId),
    /// Output is pending spent.
    /// The transaction consuming this output has been transmitted.
    TransmittedSpent(TxId),
    /// Output is pending spent.
    /// The transaction consuming this output has been detected in the mempool.
    MempoolSpent(TxId),
    /// Output is spent.
    /// The transaction consuming this output is confirmed.
    Spent(TxId),
}

impl std::fmt::Display for SpendStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpendStatus::Unspent => write!(f, "unspent"),
            SpendStatus::CalculatedSpent(txid) => write!(f, "calculated: spent in {}", txid),
            SpendStatus::TransmittedSpent(txid) => write!(f, "transmitted: spent in {}", txid),
            SpendStatus::MempoolSpent(txid) => write!(f, "mempool: spent in {}", txid),
            SpendStatus::Spent(txid) => write!(f, "confirmed: spent in {}", txid),
        }
    }
}

impl LightWallet {
    /// Returns [self::SpendStatus] for the given `output`.
    pub fn output_spend_status(&self, output: &impl OutputInterface) -> SpendStatus {
        if let Some(txid) = output.spending_transaction() {
            match self
                .wallet_transactions
                .get(&txid)
                .expect("transaction should exist in the wallet")
                .status()
            {
                ConfirmationStatus::Calculated(_) => SpendStatus::CalculatedSpent(txid),
                ConfirmationStatus::Transmitted(_) => SpendStatus::TransmittedSpent(txid),
                ConfirmationStatus::Mempool(_) => SpendStatus::MempoolSpent(txid),
                ConfirmationStatus::Confirmed(_) => SpendStatus::Spent(txid),
            }
        } else {
            SpendStatus::Unspent
        }
    }

    /// Gets all outputs of a given type in the wallet.
    pub(super) fn wallet_outputs<Op: OutputInterface>(&self) -> Vec<&Op> {
        self.wallet_transactions
            .values()
            .flat_map(|transaction| Op::transaction_outputs(transaction))
            .collect()
    }

    /// Sum the values of all outputs in the wallet which match the given `query`.
    pub fn sum_queried_output_values(&self, query: OutputQuery) -> u64 {
        self.wallet_transactions
            .values()
            .fold(0, |acc, transaction| {
                acc + self.sum_queried_transaction_output_values(transaction, query)
            })
    }

    /// Sum the values of all outputs in the `transaction` which match the given `query`.
    pub fn sum_queried_transaction_output_values(
        &self,
        transaction: &WalletTransaction,
        query: OutputQuery,
    ) -> u64 {
        let mut sum = 0;
        if query.transparent() {
            for output in transaction.transparent_coins().iter() {
                if self.query_output_spend_status(query.spend_status, output) {
                    sum += output.value();
                }
            }
        }
        if query.sapling() {
            for output in transaction.sapling_notes().iter() {
                if self.query_output_spend_status(query.spend_status, output) {
                    sum += output.value();
                }
            }
        }
        if query.orchard() {
            for output in transaction.orchard_notes().iter() {
                if self.query_output_spend_status(query.spend_status, output) {
                    sum += output.value();
                }
            }
        }
        sum
    }

    /// Returns `true` if `output` spend status matches the `query`. Otherwise, returns `false`.
    fn query_output_spend_status(
        &self,
        query: OutputSpendStatusQuery,
        output: &impl OutputInterface,
    ) -> bool {
        if let Some(txid) = output.spending_transaction() {
            match self
                .wallet_transactions
                .get(&txid)
                .expect("transaction should exist in the wallet")
                .status()
            {
                ConfirmationStatus::Confirmed(_) => query.spent,
                _confirmation_pending if query.pending_spent => true,
                _ => false,
            }
        } else {
            query.unspent
        }
    }
}

/// An interface for accessing all the common functionality of all the outputs
#[enum_dispatch::enum_dispatch(OldOutputInterface)]
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
            transparent::mocks::TransparentOutputBuilder, OldOutputInterface as _,
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

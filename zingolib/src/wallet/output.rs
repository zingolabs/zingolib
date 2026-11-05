//! All things needed to create, manaage, and use notes

use pepper_sync::wallet::KeyIdInterface;
use shardtree::store::ShardStore;
use zcash_primitives::transaction::TxId;
use zcash_protocol::PoolType;
use zcash_protocol::ShieldedPool;
use zcash_protocol::consensus::BlockHeight;

use super::LightWallet;
use super::error::WalletError;
use super::note_selection;
use pepper_sync::wallet::NoteInterface;
use pepper_sync::wallet::OutputId;
use pepper_sync::wallet::OutputInterface;
use pepper_sync::wallet::TransparentCoin;
use pepper_sync::wallet::WalletTransaction;
use query::OutputQuery;
use query::OutputSpendStatusQuery;
use zingo_status::confirmation_status::ConfirmationStatus;

pub mod query;

/// Output reference.
///
/// Identifier with pool type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct OutputRef {
    output_id: OutputId,
    pool_type: PoolType,
}

impl OutputRef {
    /// Creates new `OutputRef` from parts.
    #[must_use]
    pub fn new(output_id: OutputId, pool_type: PoolType) -> Self {
        OutputRef {
            output_id,
            pool_type,
        }
    }

    /// Output identifier.
    #[must_use]
    pub fn output_id(&self) -> OutputId {
        self.output_id
    }

    /// Output identifier.
    #[must_use]
    pub fn txid(&self) -> TxId {
        self.output_id.txid()
    }

    /// Output identifier.
    #[must_use]
    pub fn output_index(&self) -> u32 {
        self.output_id.output_index()
    }

    /// Pool type.
    #[must_use]
    pub fn pool_type(&self) -> PoolType {
        self.pool_type
    }
}

impl std::fmt::Display for OutputRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{
                output id: {}
                pool type: {}
            }}",
            self.output_id, self.pool_type
        )
    }
}

/// Spend status of an output
#[derive(Debug, Clone, Copy, PartialEq)]
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

impl SpendStatus {
    #[must_use]
    pub fn is_unspent(&self) -> bool {
        matches!(self, Self::Unspent)
    }

    #[must_use]
    pub fn is_pending_spent(&self) -> bool {
        matches!(self, Self::CalculatedSpent(_))
            || matches!(self, Self::TransmittedSpent(_))
            || matches!(self, Self::MempoolSpent(_))
    }
    #[must_use]
    pub fn is_confirmed_spent(&self) -> bool {
        matches!(self, Self::Spent(_))
    }
}

impl std::fmt::Display for SpendStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpendStatus::Unspent => write!(f, "unspent"),
            SpendStatus::CalculatedSpent(txid) => write!(f, "calculated spent in {txid}"),
            SpendStatus::TransmittedSpent(txid) => write!(f, "transmitted spent in {txid}"),
            SpendStatus::MempoolSpent(txid) => write!(f, "mempool spent in {txid}"),
            SpendStatus::Spent(txid) => write!(f, "confirmed spent in {txid}"),
        }
    }
}

impl LightWallet {
    /// Returns the transaction the given `output` belongs to.
    pub fn output_transaction(&self, output: &impl OutputInterface) -> &WalletTransaction {
        self.wallet_transactions
            .get(&output.output_id().txid())
            .expect("transaction should exist in the wallet")
    }

    /// Returns [`self::SpendStatus`] for the given `output`.
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
                ConfirmationStatus::Failed(_) => SpendStatus::Unspent,
            }
        } else {
            SpendStatus::Unspent
        }
    }

    /// Gets all outputs of a given type in the wallet.
    #[must_use]
    pub fn wallet_outputs<Op: OutputInterface>(&self) -> Vec<&Op> {
        self.wallet_transactions
            .values()
            .flat_map(|transaction| Op::transaction_outputs(transaction))
            .collect()
    }

    /// Sum the values of all outputs in the wallet which match the given `query`.
    #[must_use]
    pub fn sum_queried_output_values(&self, query: OutputQuery) -> u64 {
        self.wallet_transactions
            .values()
            .fold(0, |acc, transaction| {
                acc + self.sum_queried_transaction_output_values(transaction, query)
            })
    }

    /// Sum the values of all outputs in the `transaction` which match the given `query`.
    #[must_use]
    pub fn sum_queried_transaction_output_values(
        &self,
        transaction: &WalletTransaction,
        query: OutputQuery,
    ) -> u64 {
        let mut sum = 0;
        if query.transparent() {
            for output in transaction.transparent_coins() {
                if self.query_output_spend_status(query.spend_status, output) {
                    sum += output.value();
                }
            }
        }
        if query.sapling() {
            for output in transaction.sapling_notes() {
                if self.query_output_spend_status(query.spend_status, output) {
                    sum += output.value();
                }
            }
        }
        if query.orchard() {
            for output in transaction.orchard_notes() {
                if self.query_output_spend_status(query.spend_status, output) {
                    sum += output.value();
                }
            }
        }
        if query.ironwood() {
            for output in transaction.ironwood_notes() {
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

    /// Returns all spendable notes of the specified shielded pool and `account` confirmed at or below `anchor_height`.
    ///
    /// Any notes with output IDs in `exclude` will not be returned.
    /// Any notes without a nullifier or commitment tree position will not be returned.
    /// Any notes that the wallet cannot construct a witness for with the current sync state will not be returned.
    /// If `include_potentially_spent_notes` is `true`, notes will be included even if the wallet's current sync state
    /// cannot guarantee the notes are unspent.
    #[allow(clippy::result_large_err)]
    pub(crate) fn spendable_notes<'a, N: NoteInterface>(
        &'a self,
        anchor_height: BlockHeight,
        exclude: &'a [OutputId],
        account: zip32::AccountId,
        include_potentially_spent_notes: bool,
    ) -> Result<Vec<&'a N>, WalletError> {
        let Some(spend_horizon) = self.spend_horizon(false) else {
            return Err(WalletError::NoSyncData);
        };
        if self
            .shard_trees
            .ironwood
            .store()
            .get_checkpoint(&anchor_height)
            .expect("infallible")
            .is_none()
        {
            return Err(WalletError::CheckpointNotFound {
                shielded_protocol: ShieldedPool::Ironwood,
                height: anchor_height,
            });
        }
        if self
            .shard_trees
            .orchard
            .store()
            .get_checkpoint(&anchor_height)
            .expect("infallible")
            .is_none()
        {
            return Err(WalletError::CheckpointNotFound {
                shielded_protocol: ShieldedPool::Orchard,
                height: anchor_height,
            });
        }
        if self
            .shard_trees
            .sapling
            .store()
            .get_checkpoint(&anchor_height)
            .expect("infallible")
            .is_none()
        {
            return Err(WalletError::CheckpointNotFound {
                shielded_protocol: ShieldedPool::Sapling,
                height: anchor_height,
            });
        }

        Ok(self
            .wallet_transactions
            .values()
            .flat_map(|transaction| {
                if transaction
                    .status()
                    .is_confirmed_before_or_at(&anchor_height)
                {
                    N::transaction_outputs(transaction)
                        .iter()
                        .filter(move |&note| {
                            note.spending_transaction().is_none()
                                && note.nullifier().is_some()
                                && note.position().is_some()
                                && self.can_build_witness::<N>(
                                    transaction.status().get_height(),
                                    anchor_height,
                                )
                                && !exclude.contains(&note.output_id())
                                && note.key_id().account_id() == account
                                && (include_potentially_spent_notes
                                    || self.note_spends_confirmed(
                                        transaction.status().get_height(),
                                        spend_horizon,
                                        note.refetch_nullifier_ranges(),
                                    ))
                        })
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                }
            })
            .collect())
    }

    /// Returns all spendable transparent coins for a given `account` confirmed at or below `target_height`.
    ///
    /// Any coins from a coinbase transaction will not be returned without
    /// [`COINBASE_MATURITY_BLOCKS`](zcash_protocol::consensus::COINBASE_MATURITY_BLOCKS)
    /// additional confirmations.
    pub(crate) fn spendable_transparent_coins(
        &self,
        target_height: BlockHeight,
        allow_zero_conf_shielding: bool,
        include_potentially_spent_coins: bool,
    ) -> Vec<&TransparentCoin> {
        // TODO: add support for zero conf shielding
        assert!(
            !allow_zero_conf_shielding,
            "zero conf shielding not currently supported!"
        );

        let Some(spend_horizon) = self.spend_horizon(true) else {
            return Vec::new();
        };

        self.wallet_transactions
            .values()
            .flat_map(|transaction| {
                let additional_confirmations = transaction
                    .transaction()
                    .transparent_bundle()
                    .map_or(0, |bundle| {
                        if bundle.is_coinbase() {
                            zcash_protocol::consensus::COINBASE_MATURITY_BLOCKS
                        } else {
                            0
                        }
                    });

                if transaction
                    .status()
                    .is_confirmed_before_or_at(&(target_height - additional_confirmations))
                {
                    TransparentCoin::transaction_outputs(transaction)
                        .iter()
                        .filter(move |&coin| {
                            coin.spending_transaction().is_none()
                                && if include_potentially_spent_coins {
                                    true
                                } else {
                                    // checks all ranges with `FoundNote` priority or higher above the coin height are
                                    // scanned as all relevant transactions containing transparent spends are known and
                                    // targetted before scanning begins.
                                    transaction.status().get_height() >= spend_horizon
                                }
                        })
                        .collect()
                } else {
                    Vec::new()
                }
            })
            .collect()
    }

    #[allow(clippy::result_large_err)]
    pub(crate) fn note_candidates<N: NoteInterface + Clone>(
        &self,
        anchor_height: BlockHeight,
        account: zip32::AccountId,
        exclude: &[OutputRef],
        reserved: &[OutputId],
    ) -> Result<Vec<note_selection::Candidate<N>>, WalletError> {
        let exclude = exclude
            .iter()
            .filter(|output_ref| output_ref.pool_type() == N::POOL_TYPE)
            .map(|output_ref| OutputId::new(output_ref.txid(), output_ref.output_index()))
            .collect::<Vec<_>>();
        let certain = self
            .spendable_notes::<N>(anchor_height, &exclude, account, false)?
            .into_iter()
            .map(N::output_id)
            .collect::<Vec<_>>();
        Ok(self
            .spendable_notes::<N>(anchor_height, &exclude, account, true)?
            .into_iter()
            .map(|note| note_selection::Candidate {
                certain: certain.contains(&note.output_id()),
                reserved: reserved.contains(&note.output_id()),
                note: note.clone(),
            })
            .collect())
    }
}

// FIXME: zingo2, update for new output types
/*
#[cfg(test)]
pub mod mocks {
    //! Mock version of the struct for testing
    use zcash_client_backend::{wallet::NoteId, ShieldedPool};
    use zcash_primitives::transaction::TxId;

    use crate::{mocks::default_txid, testutils::build_method};

    /// to build a mock NoteRecordIdentifier
    pub struct NoteIdBuilder {
        txid: Option<TxId>,
        shpool: Option<ShieldedPool>,
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
        build_method!(shpool, ShieldedPool);
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
                .shpool(zcash_client_backend::ShieldedPool::Orchard)
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
        wallet::output::{
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
*/

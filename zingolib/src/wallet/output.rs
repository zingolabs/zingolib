//! All things needed to create, manaage, and use notes

use std::collections::HashSet;

use pepper_sync::sync::ScanRange;
use pepper_sync::wallet::KeyIdInterface;
use zcash_primitives::transaction::TxId;
use zcash_protocol::PoolType;
use zcash_protocol::ShieldedPool;
use zcash_protocol::consensus::BlockHeight;

use super::LightWallet;
use super::error::WalletError;
use super::note_selection;
use super::propose;
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

    #[allow(clippy::result_large_err)]
    pub(crate) fn spendable_notes<'a, N: NoteInterface>(
        &'a self,
        anchor_height: BlockHeight,
        exclude: &HashSet<OutputId>,
        account: zip32::AccountId,
        include_potentially_spent_notes: bool,
    ) -> Result<Vec<&'a N>, WalletError> {
        Ok(self
            .judged_notes::<N>(anchor_height, exclude, account)?
            .into_iter()
            .filter(|judged| judged.verdict.admits(include_potentially_spent_notes))
            .map(|judged| judged.note)
            .collect())
    }

    #[allow(clippy::result_large_err)]
    pub(crate) fn judged_notes<'a, N: NoteInterface>(
        &'a self,
        anchor_height: BlockHeight,
        exclude: &HashSet<OutputId>,
        account: zip32::AccountId,
    ) -> Result<Vec<Judged<'a, N>>, WalletError> {
        let Some(spend_horizon) = self.spend_horizon(false) else {
            return Err(WalletError::NoSyncData);
        };
        for shielded_protocol in [
            ShieldedPool::Ironwood,
            ShieldedPool::Orchard,
            ShieldedPool::Sapling,
        ] {
            if !self.checkpoint_is_retained(shielded_protocol, anchor_height) {
                return Err(WalletError::CheckpointNotFound {
                    shielded_protocol,
                    height: anchor_height,
                });
            }
        }
        let judgement = Judgement {
            account,
            exclude,
            spend_horizon,
            scan_ranges: self.sync_state.scan_ranges(),
        };
        let judgement = &judgement;
        Ok(self
            .wallet_transactions
            .values()
            .flat_map(|transaction| {
                let confirmation = self.confirmation::<N>(transaction, anchor_height);
                N::transaction_outputs(transaction)
                    .iter()
                    .map(move |note| Judged {
                        note,
                        verdict: judgement.judge(note, confirmation),
                    })
            })
            .collect())
    }

    pub(crate) fn confirmation<N: NoteInterface>(
        &self,
        transaction: &WalletTransaction,
        anchor_height: BlockHeight,
    ) -> Confirmation {
        let status = transaction.status();
        if !status.is_confirmed_before_or_at(&anchor_height) {
            return Confirmation::Pending;
        }
        let height = status.get_height();
        Confirmation::Anchored {
            height,
            witnessable: self.can_build_witness::<N>(height, anchor_height),
        }
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
        reserved: &HashSet<OutputId>,
    ) -> Result<Vec<note_selection::Candidate<N>>, WalletError> {
        let exclude = exclude
            .iter()
            .filter(|output_ref| output_ref.pool_type() == N::POOL_TYPE)
            .map(|output_ref| OutputId::new(output_ref.txid(), output_ref.output_index()))
            .collect::<HashSet<_>>();
        Ok(self
            .judged_notes::<N>(anchor_height, &exclude, account)?
            .into_iter()
            .filter_map(|judged| {
                judged
                    .verdict
                    .certainty()
                    .map(|certain| note_selection::Candidate {
                        certain,
                        reserved: reserved.contains(&judged.note.output_id()),
                        note: judged.note.clone(),
                    })
            })
            .collect())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Confirmation {
    Pending,
    Anchored {
        height: BlockHeight,
        witnessable: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    Certain,
    Uncertain,
    Pending,
    ForeignAccount,
    Excluded,
    Spent,
    NoNullifier,
    NoPosition,
    NoWitness,
}

impl Verdict {
    pub(crate) fn certainty(self) -> Option<bool> {
        match self {
            Verdict::Certain => Some(true),
            Verdict::Uncertain => Some(false),
            Verdict::Pending
            | Verdict::ForeignAccount
            | Verdict::Excluded
            | Verdict::Spent
            | Verdict::NoNullifier
            | Verdict::NoPosition
            | Verdict::NoWitness => None,
        }
    }

    pub(crate) fn admits(self, include_potentially_spent_notes: bool) -> bool {
        match self.certainty() {
            Some(true) => true,
            Some(false) => include_potentially_spent_notes,
            None => false,
        }
    }
}

pub(crate) struct Judged<'a, N> {
    pub(crate) note: &'a N,
    pub(crate) verdict: Verdict,
}

pub(crate) struct Judgement<'a> {
    pub(crate) account: zip32::AccountId,
    pub(crate) exclude: &'a HashSet<OutputId>,
    pub(crate) spend_horizon: BlockHeight,
    pub(crate) scan_ranges: &'a [ScanRange],
}

impl Judgement<'_> {
    pub(crate) fn judge<N: NoteInterface>(&self, note: &N, confirmation: Confirmation) -> Verdict {
        let Confirmation::Anchored {
            height,
            witnessable,
        } = confirmation
        else {
            return Verdict::Pending;
        };
        if note.key_id().account_id() != self.account {
            return Verdict::ForeignAccount;
        }
        if self.exclude.contains(&note.output_id()) {
            return Verdict::Excluded;
        }
        if note.spending_transaction().is_some() {
            return Verdict::Spent;
        }
        if note.nullifier().is_none() {
            return Verdict::NoNullifier;
        }
        if note.position().is_none() {
            return Verdict::NoPosition;
        }
        if !witnessable {
            return Verdict::NoWitness;
        }
        if propose::spends_confirmed(
            height,
            self.spend_horizon,
            note.refetch_nullifier_ranges(),
            self.scan_ranges,
        ) {
            Verdict::Certain
        } else {
            Verdict::Uncertain
        }
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

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashSet};

    use pepper_sync::wallet::OrchardNote;

    use super::*;
    use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;

    const ANCHOR: BlockHeight = BlockHeight::from_u32(20);
    const ACCOUNT: zip32::AccountId = zip32::AccountId::ZERO;

    fn funded_wallet() -> LightWallet {
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .tip(20)
            .orchard_note(100_000)
            .orchard_note(5_000)
            .build()
    }

    fn ids<'a>(notes: impl IntoIterator<Item = &'a OrchardNote>) -> BTreeSet<OutputId> {
        notes.into_iter().map(OutputInterface::output_id).collect()
    }

    fn candidates(wallet: &LightWallet) -> Vec<note_selection::Candidate<OrchardNote>> {
        wallet
            .note_candidates(ANCHOR, ACCOUNT, &[], &HashSet::new())
            .expect("the synthetic wallet is synced")
    }

    fn spendable(wallet: &LightWallet, include_potentially_spent: bool) -> Vec<&OrchardNote> {
        wallet
            .spendable_notes::<OrchardNote>(
                ANCHOR,
                &HashSet::new(),
                ACCOUNT,
                include_potentially_spent,
            )
            .expect("the synthetic wallet is synced")
    }

    fn mark_spent(wallet: &mut LightWallet, spent: OutputId) {
        for transaction in wallet.wallet_transactions.values_mut() {
            for note in transaction.orchard_notes_mut() {
                if note.output_id() == spent {
                    note.set_spending_transaction(Some(spent.txid()));
                }
            }
        }
    }

    // Tests that the candidates are the lenient spendable walk and the certain ones are the strict walk.
    #[test]
    fn candidates_agree_with_the_spendable_walks() {
        let wallet = funded_wallet();
        let candidates = candidates(&wallet);
        assert_eq!(candidates.len(), 2);
        assert_eq!(
            ids(candidates.iter().map(|candidate| &candidate.note)),
            ids(spendable(&wallet, true))
        );
        assert_eq!(
            ids(candidates
                .iter()
                .filter(|candidate| candidate.certain)
                .map(|candidate| &candidate.note)),
            ids(spendable(&wallet, false))
        );
    }

    // Tests that a spent note leaves both spendable walks and the candidates.
    #[test]
    fn a_spent_note_leaves_the_candidates() {
        let mut wallet = funded_wallet();
        let spent = spendable(&wallet, false)
            .into_iter()
            .find(|note| note.value() == 100_000)
            .map(OutputInterface::output_id)
            .expect("the funded wallet holds a 100_000 orchard note");
        mark_spent(&mut wallet, spent);
        assert_eq!(
            ids(spendable(&wallet, true)),
            ids(spendable(&wallet, false))
        );
        assert!(!ids(spendable(&wallet, true)).contains(&spent));
        assert!(
            candidates(&wallet)
                .iter()
                .all(|candidate| candidate.note.output_id() != spent)
        );
    }

    fn a_note(wallet: &LightWallet) -> &OrchardNote {
        spendable(wallet, false)
            .into_iter()
            .find(|note| note.value() == 100_000)
            .expect("the funded wallet holds a 100_000 orchard note")
    }

    fn judgement<'a>(wallet: &'a LightWallet, exclude: &'a HashSet<OutputId>) -> Judgement<'a> {
        Judgement {
            account: ACCOUNT,
            exclude,
            spend_horizon: wallet
                .spend_horizon(false)
                .expect("the synthetic wallet is synced"),
            scan_ranges: wallet.sync_state.scan_ranges(),
        }
    }

    const ANCHORED: Confirmation = Confirmation::Anchored {
        height: ANCHOR,
        witnessable: true,
    };

    // Tests that the single walk judges every note of the pool, and that the funded wallet's notes are all certain.
    #[test]
    fn every_note_is_judged_and_the_funded_wallet_is_certain() {
        let wallet = funded_wallet();
        let judged = wallet
            .judged_notes::<OrchardNote>(ANCHOR, &HashSet::new(), ACCOUNT)
            .expect("the synthetic wallet is synced");
        assert_eq!(judged.len(), 2);
        assert!(
            judged
                .iter()
                .all(|judged| judged.verdict == Verdict::Certain)
        );
    }

    // Tests the pure judgement fact by fact: each fact it reads has a verdict of its own.
    #[test]
    fn judge_names_the_first_failing_fact() {
        let wallet = funded_wallet();
        let note = a_note(&wallet);
        let none = HashSet::new();
        assert_eq!(
            judgement(&wallet, &none).judge(note, ANCHORED),
            Verdict::Certain
        );
        assert_eq!(
            judgement(&wallet, &none).judge(note, Confirmation::Pending),
            Verdict::Pending
        );
        assert_eq!(
            judgement(&wallet, &none).judge(
                note,
                Confirmation::Anchored {
                    height: ANCHOR,
                    witnessable: false,
                }
            ),
            Verdict::NoWitness
        );
        let excluded = HashSet::from([note.output_id()]);
        assert_eq!(
            judgement(&wallet, &excluded).judge(note, ANCHORED),
            Verdict::Excluded
        );
        let foreign = Judgement {
            account: zip32::AccountId::try_from(1).expect("1 is a valid account id"),
            ..judgement(&wallet, &none)
        };
        assert_eq!(foreign.judge(note, ANCHORED), Verdict::ForeignAccount);
        let unscanned = Judgement {
            spend_horizon: ANCHOR + 1,
            scan_ranges: &[],
            ..judgement(&wallet, &none)
        };
        assert_eq!(unscanned.judge(note, ANCHORED), Verdict::Uncertain);
    }

    // Tests that a spent note is judged spent rather than dropped from the walk.
    #[test]
    fn a_spent_note_is_judged_spent() {
        let mut wallet = funded_wallet();
        let spent = a_note(&wallet).output_id();
        mark_spent(&mut wallet, spent);
        let judged = wallet
            .judged_notes::<OrchardNote>(ANCHOR, &HashSet::new(), ACCOUNT)
            .expect("the synthetic wallet is synced");
        let verdict = judged
            .iter()
            .find(|judged| judged.note.output_id() == spent)
            .map(|judged| judged.verdict);
        assert_eq!(verdict, Some(Verdict::Spent));
    }
}

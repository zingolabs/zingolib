//! Module for reading and updating wallet data related to spending

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use tokio::sync::mpsc;

use incrementalmerkletree::{Hashable, Position};
use shardtree::{ShardTree, store::ShardStore};
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::{self, BlockHeight};
use zingo_status::confirmation_status;
use zip32::AccountId;

use crate::{
    SyncDomain,
    client::{self, FetchRequest},
    error::SyncError,
    scan::{
        DecryptedNoteData,
        transactions::{ScannedTransactions, scan_transactions},
    },
    wallet::{
        self, Ironwood, NoteInterface, NullifierMap, Orchard, OutputId, OutputInterface, Sapling,
        ScanTarget, WalletBlock, WalletTransaction,
        traits::{SyncBlocks, SyncNullifiers, SyncOutPoints, SyncShardTrees, SyncTransactions},
    },
};

use super::state;

/// The spend scan targets of each shielded pool, keyed by the nullifier of the spent note.
pub(super) struct ShieldedSpendScanTargets {
    pub(super) sapling: BTreeMap<sapling_crypto::Nullifier, ScanTarget>,
    pub(super) orchard: BTreeMap<orchard::note::Nullifier, ScanTarget>,
    pub(super) ironwood: BTreeMap<orchard::note::Nullifier, ScanTarget>,
}

impl ShieldedSpendScanTargets {
    /// Moves the spend scan targets of each pool in `other` into the same pool here.
    fn append(&mut self, other: &mut Self) {
        self.sapling.append(&mut other.sapling);
        self.orchard.append(&mut other.orchard);
        self.ironwood.append(&mut other.ironwood);
    }

    /// The scan targets of every pool.
    fn scan_targets(&self) -> impl Iterator<Item = ScanTarget> {
        self.sapling
            .values()
            .chain(self.orchard.values())
            .chain(self.ironwood.values())
            .copied()
    }
}

/// The transactions the wallet holds once `scanned_transactions` are added to it. A scanned transaction replaces the
/// wallet's record of the same transaction, which is the contract of
/// [`SyncTransactions::extend_wallet_transactions`].
fn transactions_with_scanned<'a>(
    wallet_transactions: &'a HashMap<TxId, WalletTransaction>,
    scanned_transactions: &'a HashMap<TxId, WalletTransaction>,
) -> impl Iterator<Item = &'a WalletTransaction> + Clone {
    wallet_transactions
        .iter()
        .filter(|(txid, _)| !scanned_transactions.contains_key(*txid))
        .map(|(_, transaction)| transaction)
        .chain(scanned_transactions.values())
}

/// Locates the spends of the wallet's notes for [`apply_shielded_spends`] to record in the wallet.
///
/// The wallet is only read, so a failed server request leaves it as it is. The data the scan is about to add to
/// the wallet is passed in beside it. The notes are those of the wallet's transactions and of
/// `scanned_transactions`, and a note is spent where its derived nullifier matches a nullifier in the wallet's
/// nullifier map or in `scanned_nullifiers`.
///
/// In the edge case where a spending transaction received no change, the transaction evaded trial decryption. It is
/// fetched, scanned and added to `scanned_transactions`.
pub(super) async fn locate_shielded_spends<P, W>(
    consensus_parameters: &P,
    wallet: &W,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    scanned_blocks: &BTreeMap<BlockHeight, WalletBlock>,
    scanned_transactions: &mut HashMap<TxId, WalletTransaction>,
    scanned_nullifiers: &NullifierMap,
) -> Result<ShieldedSpendScanTargets, SyncError<W::Error>>
where
    P: consensus::Parameters,
    W: SyncBlocks + SyncTransactions + SyncNullifiers,
{
    let (sapling_derived_nullifiers, orchard_derived_nullifiers, ironwood_derived_nullifiers) =
        collect_derived_nullifiers(transactions_with_scanned(
            wallet
                .get_wallet_transactions()
                .map_err(SyncError::WalletError)?,
            scanned_transactions,
        ));

    let mut spend_scan_targets = detect_shielded_spends(
        wallet.get_nullifiers().map_err(SyncError::WalletError)?,
        &sapling_derived_nullifiers,
        &orchard_derived_nullifiers,
        &ironwood_derived_nullifiers,
    );
    spend_scan_targets.append(&mut detect_shielded_spends(
        scanned_nullifiers,
        &sapling_derived_nullifiers,
        &orchard_derived_nullifiers,
        &ironwood_derived_nullifiers,
    ));

    let spending_transactions = scan_spending_transactions(
        fetch_request_sender,
        consensus_parameters,
        wallet,
        ufvks,
        spend_scan_targets.scan_targets(),
        scanned_blocks,
        scanned_transactions,
    )
    .await?;
    scanned_transactions.extend(spending_transactions);

    Ok(spend_scan_targets)
}

/// Records the spends located by [`locate_shielded_spends`] in the wallet, once the wallet holds the scanned
/// transactions, nullifiers and note commitments the spends were located with.
///
/// The shard block ranges surrounding the spends are prioritised for scanning and the spent notes are updated with
/// their spending transactions. Both are idempotent and the wallet's nullifier map is left as it is, so a spend is
/// located again on every scan until the fully scanned height passes it and the cleanup drops its nullifier from the
/// map. A failure part way through therefore leaves nothing for the next scan to miss.
pub(super) fn apply_shielded_spends<P, W>(
    consensus_parameters: &P,
    wallet: &mut W,
    spend_scan_targets: ShieldedSpendScanTargets,
) -> Result<(), W::Error>
where
    P: consensus::Parameters,
    W: SyncTransactions + SyncShardTrees,
{
    state::set_found_note_scan_ranges(
        consensus_parameters,
        wallet.get_sync_state_mut()?,
        spend_scan_targets.scan_targets(),
    );

    update_spent_notes(wallet, spend_scan_targets)
}

/// For each scan target, fetch and scan the spending transaction. The wallet is only read.
///
/// A scan target is skipped where the wallet or `scanned_transactions` already holds its transaction as confirmed.
///
/// This is only intended to be used for transactions that pay everything to external recipients and therefore evaded
/// trial decryption and transparent output scanning.
/// For targetted scanning of transactions, scan targets should be added to the wallet using [`crate::add_scan_targets`] and
/// the `FoundNote` priorities will be automatically set for scan prioritisation. Transactions with incoming notes
/// are required to be scanned in the context of a scan task to correctly derive the nullifiers and positions for
/// spending.
async fn scan_spending_transactions<L, P, W>(
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    consensus_parameters: &P,
    wallet: &W,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    scan_targets: L,
    scanned_blocks: &BTreeMap<BlockHeight, WalletBlock>,
    scanned_transactions: &HashMap<TxId, WalletTransaction>,
) -> Result<HashMap<TxId, WalletTransaction>, SyncError<W::Error>>
where
    L: Iterator<Item = ScanTarget>,
    P: consensus::Parameters,
    W: SyncBlocks + SyncTransactions,
{
    let confirmed_wallet_txids = transactions_with_scanned(
        wallet
            .get_wallet_transactions()
            .map_err(SyncError::WalletError)?,
        scanned_transactions,
    )
    .filter(|transaction| transaction.status().is_confirmed())
    .map(WalletTransaction::txid)
    .collect::<HashSet<_>>();
    let mut spending_scan_targets = BTreeSet::new();
    let mut wallet_blocks = BTreeMap::new();
    for scan_target in scan_targets {
        let block_height = scan_target.block_height();
        // a spend target names the spending transaction
        let Some(txid) = scan_target.txid() else {
            continue;
        };

        // skip if confirmed transaction already exists in the wallet
        if confirmed_wallet_txids.contains(&txid) {
            continue;
        }

        spending_scan_targets.insert(scan_target);

        let wallet_block = match wallet.get_wallet_block(block_height) {
            Ok(block) => block,
            Err(_) => match scanned_blocks.get(&block_height) {
                Some(block) => block.clone(),
                None => {
                    WalletBlock::from_compact_block(
                        consensus_parameters,
                        fetch_request_sender.clone(),
                        &client::get_compact_block(fetch_request_sender.clone(), block_height)
                            .await?,
                    )
                    .await?
                }
            },
        };

        wallet_blocks.insert(block_height, wallet_block);
    }

    let mut outpoint_map = BTreeMap::new(); // dummy outpoint map
    // spend targets are discovered by the sync engine, so a transaction the server does not serve is an error
    // and no target is dropped.
    let ScannedTransactions {
        transactions: spending_transactions,
        dropped_targets: _,
    } = scan_transactions(
        fetch_request_sender,
        consensus_parameters,
        ufvks,
        spending_scan_targets,
        DecryptedNoteData::new(),
        &wallet_blocks,
        &mut outpoint_map,
        HashMap::new(), // no need to scan transparent bundles as all relevant txs will not be evaded during scanning
    )
    .await?;

    Ok(spending_transactions)
}

/// Collects the derived nullifiers from each note of `transactions`
pub(super) fn collect_derived_nullifiers<'a>(
    transactions: impl Iterator<Item = &'a WalletTransaction> + Clone,
) -> (
    Vec<sapling_crypto::Nullifier>,
    Vec<orchard::note::Nullifier>,
    Vec<orchard::note::Nullifier>,
) {
    let sapling_nullifiers = transactions
        .clone()
        .flat_map(super::super::wallet::WalletTransaction::sapling_notes)
        .filter_map(|note| note.nullifier)
        .collect::<Vec<_>>();
    let orchard_nullifiers = transactions
        .clone()
        .flat_map(super::super::wallet::WalletTransaction::orchard_notes)
        .filter_map(|note| note.nullifier)
        .collect::<Vec<_>>();
    let ironwood_nullifiers = transactions
        .flat_map(super::super::wallet::WalletTransaction::ironwood_notes)
        .filter_map(|note| note.nullifier)
        .collect::<Vec<_>>();

    (sapling_nullifiers, orchard_nullifiers, ironwood_nullifiers)
}

/// Check if any wallet note's derived nullifiers match a nullifier in the `nullifier_map`.
///
/// The `nullifier_map` is only read. The wallet's nullifier map keeps the nullifiers of the detected spends until
/// the cleanup drops them behind the fully scanned height, so a spend is detected again on every scan until then.
pub(super) fn detect_shielded_spends(
    nullifier_map: &NullifierMap,
    sapling_derived_nullifiers: &[sapling_crypto::Nullifier],
    orchard_derived_nullifiers: &[orchard::note::Nullifier],
    ironwood_derived_nullifiers: &[orchard::note::Nullifier],
) -> ShieldedSpendScanTargets {
    ShieldedSpendScanTargets {
        sapling: detect_spends(&nullifier_map.sapling, sapling_derived_nullifiers),
        orchard: detect_spends(&nullifier_map.orchard, orchard_derived_nullifiers),
        ironwood: detect_spends(&nullifier_map.ironwood, ironwood_derived_nullifiers),
    }
}

/// Update the `spending_transaction` field of all notes where the derived nullifier matches the nullifier in the spend
/// scan target map. The items in the spend scan target map are taken directly from the nullifier map during spend detection.
/// Also removes retention marks from the shard tree when a note is spent as it no longer needs the wallet to be able
/// to construct a witness for it's note commitment.
///
/// The notes are updated in a first pass over the wallet's transactions and the marks are removed in a second pass
/// over its shard trees, so nothing is taken out of the wallet to hold both at once. A wallet that fails to hand over
/// its shard trees keeps the notes marked spent with their marks retained, and the marks are removed when the spends
/// are located again.
pub(super) fn update_spent_notes<W>(
    wallet: &mut W,
    spend_scan_targets: ShieldedSpendScanTargets,
) -> Result<(), W::Error>
where
    W: SyncTransactions + SyncShardTrees,
{
    let wallet_transactions = wallet.get_wallet_transactions_mut()?;
    let sapling_mark_removals =
        update_spent_notes_by_protocol::<Sapling>(wallet_transactions, spend_scan_targets.sapling);
    let orchard_mark_removals =
        update_spent_notes_by_protocol::<Orchard>(wallet_transactions, spend_scan_targets.orchard);
    let ironwood_mark_removals = update_spent_notes_by_protocol::<Ironwood>(
        wallet_transactions,
        spend_scan_targets.ironwood,
    );

    let shard_trees = wallet.get_shard_trees_mut()?;
    remove_spent_note_marks(&mut shard_trees.sapling, sapling_mark_removals);
    remove_spent_note_marks(&mut shard_trees.orchard, orchard_mark_removals);
    remove_spent_note_marks(&mut shard_trees.ironwood, ironwood_mark_removals);

    Ok(())
}

/// A retention mark to remove from a shard tree: the position of a spent note and the height of its spending
/// transaction.
struct MarkRemoval {
    spent_note_position: Position,
    spending_height: BlockHeight,
}

/// Sets the spending transaction of each note of `D` whose nullifier is in `spend_scan_targets`.
///
/// Returns the marks to remove from the shard tree of `D`: one for each spent note that is confirmed, has a position
/// and whose spending transaction is confirmed.
fn update_spent_notes_by_protocol<D>(
    wallet_transactions: &mut HashMap<TxId, WalletTransaction>,
    spend_scan_targets: BTreeMap<<D::Note as NoteInterface>::Nullifier, ScanTarget>,
) -> Vec<MarkRemoval>
where
    D: SyncDomain,
{
    struct SpentNote {
        position: Position,
        spending_txid: TxId,
    }

    let mut spent_notes = Vec::new();
    for transaction in wallet_transactions.values_mut() {
        let transaction_confirmed = transaction.status().is_confirmed();
        D::notes_mut(transaction).into_iter().for_each(|note| {
            if let Some(spending_txid) = note
                .nullifier()
                .and_then(|nf| spend_scan_targets.get(&nf))
                .and_then(ScanTarget::txid)
            {
                note.set_spending_transaction(Some(spending_txid));

                if transaction_confirmed && let Some(position) = note.position() {
                    spent_notes.push(SpentNote {
                        position,
                        spending_txid,
                    });
                }
            }
        });
    }

    spent_notes
        .into_iter()
        .filter_map(|spent_note| {
            wallet_transactions
                .get(&spent_note.spending_txid)
                .and_then(|spending_tx| spending_tx.status().get_confirmed_height())
                .map(|spending_height| MarkRemoval {
                    spent_note_position: spent_note.position,
                    spending_height,
                })
        })
        .collect()
}

/// Removes the retention marks of `mark_removals` from `shard_tree`, as of the spending heights.
fn remove_spent_note_marks<S, const DEPTH: u8, const SHARD_HEIGHT: u8>(
    shard_tree: &mut ShardTree<S, DEPTH, SHARD_HEIGHT>,
    mark_removals: Vec<MarkRemoval>,
) where
    S: ShardStore<CheckpointId = BlockHeight>,
    S::H: Clone + PartialEq + Hashable,
{
    for mark_removal in mark_removals {
        shard_tree
            .remove_mark(
                mark_removal.spent_note_position,
                Some(&mark_removal.spending_height),
            )
            .expect("infallible");
    }
}

fn pool_spends<Op: OutputInterface>(
    transaction: &WalletTransaction,
    wallet_transactions: &HashMap<TxId, WalletTransaction>,
) -> Vec<OutputId> {
    let inputs = Op::transaction_inputs(transaction);
    if inputs.is_empty() {
        return Vec::new();
    }
    wallet_transactions
        .values()
        .flat_map(Op::transaction_outputs)
        .filter(|output| {
            output
                .spend_link()
                .is_some_and(|link| inputs.contains(&&link))
        })
        .map(|output| output.output_id())
        .collect()
}

pub(super) fn transaction_spends(
    transaction: &WalletTransaction,
    wallet_transactions: &HashMap<TxId, WalletTransaction>,
) -> Vec<OutputId> {
    [
        pool_spends::<wallet::TransparentCoin>(transaction, wallet_transactions),
        pool_spends::<wallet::SaplingNote>(transaction, wallet_transactions),
        pool_spends::<wallet::OrchardNote>(transaction, wallet_transactions),
        pool_spends::<wallet::IronwoodNote>(transaction, wallet_transactions),
    ]
    .concat()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[allow(missing_docs)]
pub enum PendingStage {
    Calculated,
    Transmitted,
    Mempool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[allow(missing_docs)]
pub struct PendingSpend {
    pub stage: PendingStage,
    pub txid: TxId,
}

enum SpenderStanding {
    Pending(PendingStage),
    Confirmed,
    Failed,
}

impl From<confirmation_status::ConfirmationStatus> for SpenderStanding {
    fn from(status: confirmation_status::ConfirmationStatus) -> Self {
        match status {
            confirmation_status::ConfirmationStatus::Calculated(_) => {
                Self::Pending(PendingStage::Calculated)
            }
            confirmation_status::ConfirmationStatus::Transmitted(_) => {
                Self::Pending(PendingStage::Transmitted)
            }
            confirmation_status::ConfirmationStatus::Mempool(_) => {
                Self::Pending(PendingStage::Mempool)
            }
            confirmation_status::ConfirmationStatus::Confirmed(_) => Self::Confirmed,
            confirmation_status::ConfirmationStatus::Failed(_) => Self::Failed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum SpendState {
    Unspent,
    PendingSpent(PendingSpend),
    ContestedPendingSpent {
        leading: PendingSpend,
        rivals: BTreeSet<PendingSpend>,
    },
    ConfirmedSpent(TxId),
    SpentByUnheldTransaction(TxId),
}

#[allow(missing_docs)]
impl SpendState {
    #[must_use]
    pub fn is_unspent(&self) -> bool {
        matches!(self, Self::Unspent)
    }

    #[must_use]
    pub fn spender(&self) -> Option<TxId> {
        match self {
            Self::Unspent => None,
            Self::PendingSpent(spend) | Self::ContestedPendingSpent { leading: spend, .. } => {
                Some(spend.txid)
            }
            Self::ConfirmedSpent(txid) | Self::SpentByUnheldTransaction(txid) => Some(*txid),
        }
    }

    fn of_pending_spends(spends: BTreeSet<PendingSpend>) -> Self {
        let mut most_advanced_first = spends.into_iter().rev();
        let Some(leading) = most_advanced_first.next() else {
            return Self::Unspent;
        };
        let rivals = most_advanced_first.collect::<BTreeSet<_>>();
        if rivals.is_empty() {
            Self::PendingSpent(leading)
        } else {
            Self::ContestedPendingSpent { leading, rivals }
        }
    }
}

#[allow(missing_docs)]
pub struct SpendView<'a> {
    wallet_transactions: &'a HashMap<TxId, WalletTransaction>,
    pending_spends: BTreeMap<OutputId, BTreeSet<PendingSpend>>,
}

#[allow(missing_docs)]
impl<'a> SpendView<'a> {
    #[must_use]
    pub fn new(wallet_transactions: &'a HashMap<TxId, WalletTransaction>) -> Self {
        let pending_spends = wallet_transactions
            .values()
            .filter_map(|transaction| match transaction.status().into() {
                SpenderStanding::Pending(stage) => Some((
                    transaction,
                    PendingSpend {
                        stage,
                        txid: transaction.txid(),
                    },
                )),
                SpenderStanding::Confirmed | SpenderStanding::Failed => None,
            })
            .flat_map(|(transaction, spend)| {
                transaction_spends(transaction, wallet_transactions)
                    .into_iter()
                    .map(move |output_id| (output_id, spend))
            })
            .fold(
                BTreeMap::<OutputId, BTreeSet<PendingSpend>>::new(),
                |mut pending_spends, (output_id, spend)| {
                    pending_spends.entry(output_id).or_default().insert(spend);
                    pending_spends
                },
            );
        Self {
            wallet_transactions,
            pending_spends,
        }
    }

    #[must_use]
    pub fn state<Op: OutputInterface>(&self, output: &Op) -> SpendState {
        let derived = self
            .pending_spends
            .get(&output.output_id())
            .cloned()
            .unwrap_or_default();
        let Some(txid) = output.spending_transaction() else {
            return SpendState::of_pending_spends(derived);
        };
        let Some(spender) = self.wallet_transactions.get(&txid) else {
            return SpendState::SpentByUnheldTransaction(txid);
        };
        match spender.status().into() {
            SpenderStanding::Confirmed => SpendState::ConfirmedSpent(txid),
            SpenderStanding::Failed => SpendState::of_pending_spends(derived),
            SpenderStanding::Pending(stage) => SpendState::of_pending_spends(
                derived
                    .into_iter()
                    .chain([PendingSpend { stage, txid }])
                    .collect(),
            ),
        }
    }

    #[must_use]
    pub fn derives_pending_spend(&self, output_id: OutputId, txid: TxId) -> bool {
        self.pending_spends
            .get(&output_id)
            .is_some_and(|spends| spends.iter().any(|spend| spend.txid == txid))
    }
}

/// Locates the spends of the wallet's coins for [`apply_transparent_spends`] to record in the wallet.
///
/// The wallet is only read, so a failed server request leaves it as it is. The data the scan is about to add to
/// the wallet is passed in beside it. The coins are those of the wallet's transactions and of
/// `scanned_transactions`, and a coin is spent where its output id matches an output id in the wallet's outpoint map
/// or in `scanned_outpoints`.
///
/// In the edge case where a spending transaction received no change, the transaction evaded transparent output
/// scanning. It is fetched, scanned and added to `scanned_transactions`.
pub(super) async fn locate_transparent_spends<P, W>(
    consensus_parameters: &P,
    wallet: &W,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    scanned_blocks: &BTreeMap<BlockHeight, WalletBlock>,
    scanned_transactions: &mut HashMap<TxId, WalletTransaction>,
    scanned_outpoints: &BTreeMap<OutputId, ScanTarget>,
) -> Result<BTreeMap<OutputId, ScanTarget>, SyncError<W::Error>>
where
    P: consensus::Parameters,
    W: SyncBlocks + SyncTransactions + SyncOutPoints,
{
    let transparent_output_ids = collect_transparent_output_ids(transactions_with_scanned(
        wallet
            .get_wallet_transactions()
            .map_err(SyncError::WalletError)?,
        scanned_transactions,
    ));

    let mut transparent_spend_scan_targets = detect_spends(
        wallet.get_outpoints().map_err(SyncError::WalletError)?,
        &transparent_output_ids,
    );
    transparent_spend_scan_targets.append(&mut detect_spends(
        scanned_outpoints,
        &transparent_output_ids,
    ));

    let spending_transactions = scan_spending_transactions(
        fetch_request_sender,
        consensus_parameters,
        wallet,
        ufvks,
        transparent_spend_scan_targets.values().copied(),
        scanned_blocks,
        scanned_transactions,
    )
    .await?;
    scanned_transactions.extend(spending_transactions);

    Ok(transparent_spend_scan_targets)
}

/// Records the spends located by [`locate_transparent_spends`] in the wallet, once the wallet holds the scanned
/// transactions and outpoints the spends were located with.
///
/// The spent coins are updated with their spending transactions. The wallet's outpoint map is left as it is, so a
/// spend is located again on every scan until the fully scanned height passes it and the cleanup drops its output id
/// from the map.
pub(super) fn apply_transparent_spends<W>(
    wallet: &mut W,
    transparent_spend_scan_targets: BTreeMap<OutputId, ScanTarget>,
) -> Result<(), W::Error>
where
    W: SyncTransactions,
{
    update_spent_coins(
        wallet.get_wallet_transactions_mut()?,
        transparent_spend_scan_targets,
    );

    Ok(())
}

/// Collects the output ids from each coin of `transactions`
pub(super) fn collect_transparent_output_ids<'a>(
    transactions: impl Iterator<Item = &'a WalletTransaction>,
) -> Vec<OutputId> {
    transactions
        .flat_map(super::super::wallet::WalletTransaction::transparent_coins)
        .map(|coin| coin.output_id)
        .collect()
}

pub(super) fn detect_spends<K: Ord + Copy>(
    spend_map: &BTreeMap<K, ScanTarget>,
    wallet_keys: &[K],
) -> BTreeMap<K, ScanTarget> {
    wallet_keys
        .iter()
        .filter_map(|key| Some((*key, *spend_map.get(key)?)))
        .collect()
}

/// Update the spending transaction for all coins where the output id matches the output id in the spend scan target map.
/// The items in the spend scan target map are taken directly from the outpoint map during spend detection.
pub(super) fn update_spent_coins(
    wallet_transactions: &mut HashMap<TxId, WalletTransaction>,
    transparent_spend_scan_targets: BTreeMap<OutputId, ScanTarget>,
) {
    wallet_transactions
        .values_mut()
        .flat_map(|tx| tx.transparent_coins_mut())
        .for_each(|coin| {
            if let Some(spending_txid) = transparent_spend_scan_targets
                .get(&coin.output_id)
                .and_then(ScanTarget::txid)
            {
                coin.spending_transaction = Some(spending_txid);
            }
        });
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use zcash_primitives::transaction::{TransactionData, TxVersion};
    use zcash_protocol::consensus::BranchId;
    use zcash_transparent::address::Script;
    use zcash_transparent::bundle::{Authorized, OutPoint, TxIn};
    use zcash_transparent::keys::NonHardenedChildIndex;
    use zingo_status::confirmation_status::ConfirmationStatus;
    use zip32::AccountId;

    use crate::keys::transparent::{TransparentAddressId, TransparentScope};
    use crate::wallet::{OutputId, TransparentCoin, WalletTransaction};

    const FUNDING_TXID_BYTES: [u8; 32] = [1; 32];
    const UNHELD_TXID_BYTES: [u8; 32] = [9; 32];
    const FUNDING_OUTPUT_INDEX: u32 = 0;
    const COIN_VALUE: u64 = 100_000;
    const FUNDING_HEIGHT: u32 = 10;
    const FIRST_SPENDER_EXPIRY: u32 = 11;
    const SECOND_SPENDER_EXPIRY: u32 = 12;
    const WALLET_REBUILDS: usize = 64;

    fn funding_coin_id() -> OutputId {
        OutputId::new(TxId::from_bytes(FUNDING_TXID_BYTES), FUNDING_OUTPUT_INDEX)
    }

    fn funding_record(spending_transaction: Option<TxId>) -> WalletTransaction {
        let coin = TransparentCoin {
            output_id: funding_coin_id(),
            key_id: TransparentAddressId::new(
                AccountId::ZERO,
                TransparentScope::External,
                NonHardenedChildIndex::ZERO,
            ),
            address: String::new(),
            script: Script::default(),
            value: zcash_protocol::value::Zatoshis::const_from_u64(COIN_VALUE),
            spending_transaction,
        };
        WalletTransaction::new_for_test(
            TxId::from_bytes(FUNDING_TXID_BYTES),
            ConfirmationStatus::Confirmed(FUNDING_HEIGHT.into()),
        )
        .with_transparent_coins_for_test(vec![coin])
    }

    fn spender_record(
        status: ConfirmationStatus,
        expiry_height: u32,
    ) -> std::io::Result<WalletTransaction> {
        let bundle = zcash_transparent::bundle::Bundle {
            vin: vec![TxIn::from_parts(
                OutPoint::new(FUNDING_TXID_BYTES, FUNDING_OUTPUT_INDEX),
                Script::default(),
                u32::MAX,
            )],
            vout: Vec::new(),
            authorization: Authorized,
        };
        let transaction = TransactionData::from_parts(
            TxVersion::V4,
            BranchId::Sapling,
            0,
            BlockHeight::from_u32(expiry_height),
            Some(bundle),
            None,
            None,
            None,
        )
        .freeze()?;
        Ok(WalletTransaction {
            txid: transaction.txid(),
            status,
            transaction,
            datetime: 0,
            transparent_coins: Vec::new(),
            sapling_notes: Vec::new(),
            orchard_notes: Vec::new(),
            ironwood_notes: Vec::new(),
            outgoing_sapling_notes: Vec::new(),
            outgoing_orchard_notes: Vec::new(),
            outgoing_ironwood_notes: Vec::new(),
        })
    }

    fn wallet_of(
        records: impl IntoIterator<Item = WalletTransaction>,
    ) -> HashMap<TxId, WalletTransaction> {
        records
            .into_iter()
            .map(|record| (record.txid(), record))
            .collect()
    }

    fn coin_states(wallet_transactions: &HashMap<TxId, WalletTransaction>) -> Vec<SpendState> {
        let spends = SpendView::new(wallet_transactions);
        wallet_transactions
            .values()
            .flat_map(WalletTransaction::transparent_coins)
            .map(|coin| spends.state(coin))
            .collect()
    }

    fn first_spender(status: ConfirmationStatus) -> std::io::Result<WalletTransaction> {
        spender_record(status, FIRST_SPENDER_EXPIRY)
    }

    fn second_spender(status: ConfirmationStatus) -> std::io::Result<WalletTransaction> {
        spender_record(status, SECOND_SPENDER_EXPIRY)
    }

    #[test]
    fn an_output_that_no_transaction_spends_is_unspent() {
        assert_eq!(
            coin_states(&wallet_of([funding_record(None)])),
            vec![SpendState::Unspent]
        );
    }

    #[test]
    fn the_input_of_one_pending_transaction_is_pending_spent_at_its_stage() -> std::io::Result<()> {
        for (status, stage) in [
            (
                ConfirmationStatus::Calculated(FIRST_SPENDER_EXPIRY.into()),
                PendingStage::Calculated,
            ),
            (
                ConfirmationStatus::Transmitted(FIRST_SPENDER_EXPIRY.into()),
                PendingStage::Transmitted,
            ),
            (
                ConfirmationStatus::Mempool(FIRST_SPENDER_EXPIRY.into()),
                PendingStage::Mempool,
            ),
        ] {
            let spender = first_spender(status)?;
            let txid = spender.txid();
            assert_eq!(
                coin_states(&wallet_of([funding_record(None), spender])),
                vec![SpendState::PendingSpent(PendingSpend { stage, txid })]
            );
        }
        Ok(())
    }

    #[test]
    fn the_input_of_a_confirmed_or_failed_transaction_is_unspent_without_a_recorded_spend()
    -> std::io::Result<()> {
        for status in [
            ConfirmationStatus::Confirmed(FIRST_SPENDER_EXPIRY.into()),
            ConfirmationStatus::Failed(FIRST_SPENDER_EXPIRY.into()),
        ] {
            assert_eq!(
                coin_states(&wallet_of([funding_record(None), first_spender(status)?])),
                vec![SpendState::Unspent]
            );
        }
        Ok(())
    }

    #[test]
    fn two_pending_spenders_of_one_output_are_contested_the_same_way_on_every_rebuild()
    -> std::io::Result<()> {
        let transmitted = ConfirmationStatus::Transmitted(FIRST_SPENDER_EXPIRY.into());
        let in_mempool = ConfirmationStatus::Mempool(SECOND_SPENDER_EXPIRY.into());
        let contested = SpendState::ContestedPendingSpent {
            leading: PendingSpend {
                stage: PendingStage::Mempool,
                txid: second_spender(in_mempool)?.txid(),
            },
            rivals: BTreeSet::from([PendingSpend {
                stage: PendingStage::Transmitted,
                txid: first_spender(transmitted)?.txid(),
            }]),
        };
        for _ in 0..WALLET_REBUILDS {
            assert_eq!(
                coin_states(&wallet_of([
                    funding_record(None),
                    first_spender(transmitted)?,
                    second_spender(in_mempool)?,
                ])),
                vec![contested.clone()]
            );
        }
        Ok(())
    }

    #[test]
    fn a_recorded_spend_reads_by_the_standing_of_its_transaction() -> std::io::Result<()> {
        let confirmed = first_spender(ConfirmationStatus::Confirmed(FIRST_SPENDER_EXPIRY.into()))?;
        let confirmed_txid = confirmed.txid();
        assert_eq!(
            coin_states(&wallet_of([
                funding_record(Some(confirmed_txid)),
                confirmed
            ])),
            vec![SpendState::ConfirmedSpent(confirmed_txid)]
        );

        let failed = first_spender(ConfirmationStatus::Failed(FIRST_SPENDER_EXPIRY.into()))?;
        assert_eq!(
            coin_states(&wallet_of([funding_record(Some(failed.txid())), failed])),
            vec![SpendState::Unspent]
        );

        let unheld_txid = TxId::from_bytes(UNHELD_TXID_BYTES);
        assert_eq!(
            coin_states(&wallet_of([funding_record(Some(unheld_txid))])),
            vec![SpendState::SpentByUnheldTransaction(unheld_txid)]
        );
        Ok(())
    }

    #[test]
    fn a_confirmed_recorded_spend_outranks_a_pending_spender() -> std::io::Result<()> {
        let confirmed = first_spender(ConfirmationStatus::Confirmed(FIRST_SPENDER_EXPIRY.into()))?;
        let confirmed_txid = confirmed.txid();
        assert_eq!(
            coin_states(&wallet_of([
                funding_record(Some(confirmed_txid)),
                confirmed,
                second_spender(ConfirmationStatus::Mempool(SECOND_SPENDER_EXPIRY.into()))?,
            ])),
            vec![SpendState::ConfirmedSpent(confirmed_txid)]
        );
        Ok(())
    }
}

//! Module for reading and updating wallet data related to spending

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use tokio::sync::mpsc;

use incrementalmerkletree::{Hashable, Position};
use shardtree::{ShardTree, store::ShardStore};
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::transaction::TxId;
use zcash_protocol::{
    ShieldedPool,
    consensus::{self, BlockHeight},
};
use zip32::AccountId;

use crate::{
    SyncDomain,
    client::{self, FetchRequest},
    error::SyncError,
    scan::{DecryptedNoteData, transactions::scan_transactions},
    wallet::{
        Ironwood, NoteInterface, NullifierMap, Orchard, OutputId, OutputInterface, Sapling,
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
    /// Whether every pool is without a spend.
    pub(super) fn is_empty(&self) -> bool {
        self.scan_targets().next().is_none()
    }

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
/// wallet's record of the same transaction.
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
    let sync_state = wallet.get_sync_state_mut()?;
    state::set_found_note_scan_ranges(
        consensus_parameters,
        sync_state,
        ShieldedPool::Sapling,
        spend_scan_targets.sapling.values().copied(),
    );
    state::set_found_note_scan_ranges(
        consensus_parameters,
        sync_state,
        ShieldedPool::Orchard,
        spend_scan_targets.orchard.values().copied(),
    );
    state::set_found_note_scan_ranges(
        consensus_parameters,
        sync_state,
        ShieldedPool::Ironwood,
        spend_scan_targets.ironwood.values().copied(),
    );

    update_spent_notes(wallet, spend_scan_targets, true)
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
        let block_height = scan_target.block_height;
        let txid = scan_target.txid;

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
    let spending_transactions = scan_transactions(
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
    remove_marks: bool,
) -> Result<(), W::Error>
where
    W: SyncTransactions + SyncShardTrees,
{
    let wallet_transactions = wallet.get_wallet_transactions_mut()?;
    let sapling_mark_removals = update_spent_notes_by_protocol::<Sapling>(
        wallet_transactions,
        spend_scan_targets.sapling,
        remove_marks,
    );
    let orchard_mark_removals = update_spent_notes_by_protocol::<Orchard>(
        wallet_transactions,
        spend_scan_targets.orchard,
        remove_marks,
    );
    let ironwood_mark_removals = update_spent_notes_by_protocol::<Ironwood>(
        wallet_transactions,
        spend_scan_targets.ironwood,
        remove_marks,
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
/// Returns the marks to remove from the shard tree of `D`, if `remove_marks` is set: one for each spent note that is
/// confirmed, has a position and whose spending transaction is confirmed.
fn update_spent_notes_by_protocol<D>(
    wallet_transactions: &mut HashMap<TxId, WalletTransaction>,
    spend_scan_targets: BTreeMap<<D::Note as NoteInterface>::Nullifier, ScanTarget>,
    remove_marks: bool,
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
            if let Some(scan_target) = note.nullifier().and_then(|nf| spend_scan_targets.get(&nf)) {
                note.set_spending_transaction(Some(scan_target.txid));

                if remove_marks
                    && transaction_confirmed
                    && let Some(position) = note.position()
                {
                    spent_notes.push(SpentNote {
                        position,
                        spending_txid: scan_target.txid,
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
            if let Some(scan_target) = transparent_spend_scan_targets.get(&coin.output_id) {
                coin.spending_transaction = Some(scan_target.txid);
            }
        });
}

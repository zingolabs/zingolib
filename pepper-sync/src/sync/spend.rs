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
    witness::SHARD_HEIGHT,
};

use super::state;

/// The spend scan targets of each shielded pool, keyed by the nullifier of the spent note.
pub(super) type ShieldedSpendScanTargets = (
    BTreeMap<sapling_crypto::Nullifier, ScanTarget>,
    BTreeMap<orchard::note::Nullifier, ScanTarget>,
    BTreeMap<orchard::note::Nullifier, ScanTarget>,
);

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

    let (
        mut sapling_spend_scan_targets,
        mut orchard_spend_scan_targets,
        mut ironwood_spend_scan_targets,
    ) = detect_shielded_spends(
        wallet.get_nullifiers().map_err(SyncError::WalletError)?,
        &sapling_derived_nullifiers,
        &orchard_derived_nullifiers,
        &ironwood_derived_nullifiers,
    );
    let (
        mut scanned_sapling_spend_scan_targets,
        mut scanned_orchard_spend_scan_targets,
        mut scanned_ironwood_spend_scan_targets,
    ) = detect_shielded_spends(
        scanned_nullifiers,
        &sapling_derived_nullifiers,
        &orchard_derived_nullifiers,
        &ironwood_derived_nullifiers,
    );
    sapling_spend_scan_targets.append(&mut scanned_sapling_spend_scan_targets);
    orchard_spend_scan_targets.append(&mut scanned_orchard_spend_scan_targets);
    ironwood_spend_scan_targets.append(&mut scanned_ironwood_spend_scan_targets);

    let spending_transactions = scan_spending_transactions(
        fetch_request_sender,
        consensus_parameters,
        wallet,
        ufvks,
        sapling_spend_scan_targets
            .values()
            .chain(orchard_spend_scan_targets.values())
            .chain(ironwood_spend_scan_targets.values())
            .copied(),
        scanned_blocks,
        scanned_transactions,
    )
    .await?;
    scanned_transactions.extend(spending_transactions);

    Ok((
        sapling_spend_scan_targets,
        orchard_spend_scan_targets,
        ironwood_spend_scan_targets,
    ))
}

/// Records the spends located by [`locate_shielded_spends`] in the wallet, once the wallet holds the scanned
/// transactions, nullifiers and note commitments the spends were located with.
///
/// The nullifiers of the spends are removed from the wallet's nullifier map, the shard block ranges surrounding the
/// spends are prioritised for scanning and the spent notes are updated with their spending transactions.
pub(super) fn apply_shielded_spends<P, W>(
    consensus_parameters: &P,
    wallet: &mut W,
    (sapling_spend_scan_targets, orchard_spend_scan_targets, ironwood_spend_scan_targets): ShieldedSpendScanTargets,
) -> Result<(), W::Error>
where
    P: consensus::Parameters,
    W: SyncTransactions + SyncNullifiers + SyncShardTrees,
{
    remove_spent_nullifiers(
        wallet.get_nullifiers_mut()?,
        &sapling_spend_scan_targets,
        &orchard_spend_scan_targets,
        &ironwood_spend_scan_targets,
    );

    let sync_state = wallet.get_sync_state_mut()?;
    state::set_found_note_scan_ranges(
        consensus_parameters,
        sync_state,
        ShieldedPool::Sapling,
        sapling_spend_scan_targets.values().copied(),
    );
    state::set_found_note_scan_ranges(
        consensus_parameters,
        sync_state,
        ShieldedPool::Orchard,
        orchard_spend_scan_targets.values().copied(),
    );
    state::set_found_note_scan_ranges(
        consensus_parameters,
        sync_state,
        ShieldedPool::Ironwood,
        ironwood_spend_scan_targets.values().copied(),
    );

    update_spent_notes(
        wallet,
        sapling_spend_scan_targets,
        orchard_spend_scan_targets,
        ironwood_spend_scan_targets,
        true,
    )
}

/// Removes the nullifiers of detected spends from `nullifier_map`. The spent notes hold the spend from here on.
pub(super) fn remove_spent_nullifiers(
    nullifier_map: &mut NullifierMap,
    sapling_spend_scan_targets: &BTreeMap<sapling_crypto::Nullifier, ScanTarget>,
    orchard_spend_scan_targets: &BTreeMap<orchard::note::Nullifier, ScanTarget>,
    ironwood_spend_scan_targets: &BTreeMap<orchard::note::Nullifier, ScanTarget>,
) {
    for nullifier in sapling_spend_scan_targets.keys() {
        nullifier_map.sapling.remove(nullifier);
    }
    for nullifier in orchard_spend_scan_targets.keys() {
        nullifier_map.orchard.remove(nullifier);
    }
    for nullifier in ironwood_spend_scan_targets.keys() {
        nullifier_map.ironwood.remove(nullifier);
    }
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
/// The `nullifier_map` is only read, so the wallet's nullifier map keeps the nullifiers of the detected spends
/// until the spends are recorded in the wallet. See [`remove_spent_nullifiers`].
pub(super) fn detect_shielded_spends(
    nullifier_map: &NullifierMap,
    sapling_derived_nullifiers: &[sapling_crypto::Nullifier],
    orchard_derived_nullifiers: &[orchard::note::Nullifier],
    ironwood_derived_nullifiers: &[orchard::note::Nullifier],
) -> ShieldedSpendScanTargets {
    let sapling_spend_scan_targets = sapling_derived_nullifiers
        .iter()
        .filter_map(|nf| Some((*nf, *nullifier_map.sapling.get(nf)?)))
        .collect();
    let orchard_spend_scan_targets = orchard_derived_nullifiers
        .iter()
        .filter_map(|nf| Some((*nf, *nullifier_map.orchard.get(nf)?)))
        .collect();
    let ironwood_spend_scan_targets = ironwood_derived_nullifiers
        .iter()
        .filter_map(|nf| Some((*nf, *nullifier_map.ironwood.get(nf)?)))
        .collect();

    (
        sapling_spend_scan_targets,
        orchard_spend_scan_targets,
        ironwood_spend_scan_targets,
    )
}

/// Update the `spending_transaction` field of all notes where the derived nullifier matches the nullifier in the spend
/// scan target map. The items in the spend scan target map are taken directly from the nullifier map during spend detection.
/// Also removes retention marks from the shard tree when a note is spent as it no longer needs the wallet to be able
/// to construct a witness for it's note commitment.
pub(super) fn update_spent_notes<W>(
    wallet: &mut W,
    sapling_spend_scan_targets: BTreeMap<sapling_crypto::Nullifier, ScanTarget>,
    orchard_spend_scan_targets: BTreeMap<orchard::note::Nullifier, ScanTarget>,
    ironwood_spend_scan_targets: BTreeMap<orchard::note::Nullifier, ScanTarget>,
    remove_marks: bool,
) -> Result<(), W::Error>
where
    W: SyncTransactions + SyncShardTrees,
{
    let mut shard_trees = std::mem::take(wallet.get_shard_trees_mut()?);
    let wallet_transactions = wallet.get_wallet_transactions_mut()?;
    update_spent_notes_by_protocol::<
        Sapling,
        { sapling_crypto::NOTE_COMMITMENT_TREE_DEPTH },
        { SHARD_HEIGHT },
    >(
        wallet_transactions,
        &mut shard_trees.sapling,
        sapling_spend_scan_targets,
        remove_marks,
    );
    update_spent_notes_by_protocol::<
        Orchard,
        { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 },
        { SHARD_HEIGHT },
    >(
        wallet_transactions,
        &mut shard_trees.orchard,
        orchard_spend_scan_targets,
        remove_marks,
    );
    update_spent_notes_by_protocol::<
        Ironwood,
        { orchard::NOTE_COMMITMENT_TREE_DEPTH as u8 },
        { SHARD_HEIGHT },
    >(
        wallet_transactions,
        &mut shard_trees.ironwood,
        ironwood_spend_scan_targets,
        remove_marks,
    );
    *wallet.get_shard_trees_mut()? = shard_trees;

    Ok(())
}

fn update_spent_notes_by_protocol<D, const DEPTH: u8, const SHARD_HEIGHT: u8>(
    wallet_transactions: &mut HashMap<TxId, WalletTransaction>,
    shard_tree: &mut ShardTree<D::ShardStore, DEPTH, SHARD_HEIGHT>,
    spend_scan_targets: BTreeMap<<D::Note as NoteInterface>::Nullifier, ScanTarget>,
    remove_marks: bool,
) where
    D: SyncDomain,
    <D::ShardStore as ShardStore>::H: Clone + PartialEq + Hashable,
    <D::ShardStore as ShardStore>::CheckpointId: Copy + std::fmt::Debug + PartialOrd + Ord,
{
    struct MarkRemovalData {
        spent_note_position: Position,
        spending_txid: TxId,
    }

    let mut mark_removals = Vec::new();
    for transaction in wallet_transactions.values_mut() {
        let transaction_confirmed = transaction.status().is_confirmed();
        D::notes_mut(transaction).into_iter().for_each(|note| {
            if let Some(scan_target) = note.nullifier().and_then(|nf| spend_scan_targets.get(&nf)) {
                note.set_spending_transaction(Some(scan_target.txid));

                if remove_marks
                    && transaction_confirmed
                    && let Some(position) = note.position()
                {
                    mark_removals.push(MarkRemovalData {
                        spent_note_position: position,
                        spending_txid: scan_target.txid,
                    });
                }
            }
        });
    }
    for mark_removal in mark_removals {
        if let Some(spending_height) = wallet_transactions
            .get(&mark_removal.spending_txid)
            .and_then(|spending_tx| spending_tx.status().get_confirmed_height())
        {
            shard_tree
                .remove_mark(mark_removal.spent_note_position, Some(&spending_height))
                .expect("infallible");
        }
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

    let mut transparent_spend_scan_targets = detect_transparent_spends(
        wallet.get_outpoints().map_err(SyncError::WalletError)?,
        &transparent_output_ids,
    );
    transparent_spend_scan_targets.append(&mut detect_transparent_spends(
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
/// The output ids of the spends are removed from the wallet's outpoint map and the spent coins are updated with their
/// spending transactions.
pub(super) fn apply_transparent_spends<W>(
    wallet: &mut W,
    transparent_spend_scan_targets: BTreeMap<OutputId, ScanTarget>,
) -> Result<(), W::Error>
where
    W: SyncTransactions + SyncOutPoints,
{
    let outpoint_map = wallet.get_outpoints_mut()?;
    for output_id in transparent_spend_scan_targets.keys() {
        outpoint_map.remove(output_id);
    }
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

/// Check if any wallet coin's output id match an outpoint in the `outpoint_map`.
///
/// The `outpoint_map` is only read, so the wallet's outpoint map keeps the output ids of the detected spends until
/// the spends are recorded in the wallet.
pub(super) fn detect_transparent_spends(
    outpoint_map: &BTreeMap<OutputId, ScanTarget>,
    transparent_output_ids: &[OutputId],
) -> BTreeMap<OutputId, ScanTarget> {
    transparent_output_ids
        .iter()
        .filter_map(|output_id| Some((*output_id, *outpoint_map.get(output_id)?)))
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

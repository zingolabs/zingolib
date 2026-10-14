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

// REPRO: claim `remove-mark-bool-discarded`. `update_spent_notes_by_protocol`
// calls `ShardTree::remove_mark(position, Some(&spending_height))` and drops
// the returned `bool`. shardtree 0.7.1 reports two silent outcomes through
// that bool: `Ok(false)` when the spending height is at or above the oldest
// checkpoint id but no checkpoint with exactly that id exists (the mark is
// kept and nothing is recorded), and `Ok(true)` with direct removal when the
// spending height is below the oldest checkpoint id (no checkpoint records
// the removal, so a later rollback cannot restore it). These tests pin the
// library facts and then drive the crate's spend path into the first case.
#[cfg(test)]
mod remove_mark_outcome_repro {
    use std::collections::HashMap;

    use incrementalmerkletree::{Marking, Position, Retention};
    use sapling_crypto::value::NoteValue;
    use shardtree::{RetentionFlags, ShardTree, store::ShardStore};
    use zcash_primitives::transaction::TxId;
    use zcash_protocol::{consensus::BlockHeight, memo::Memo};
    use zingo_status::confirmation_status::ConfirmationStatus;

    use crate::{
        mocks::MockWalletBuilder,
        shardtree_ext::ShardTreeExt,
        sync::spend,
        wallet::{
            NullifierMap, OutputId, ScanTarget, ShardTrees, WalletNote, WalletTransaction,
            empty_shard_tree,
            traits::{SyncNullifiers, SyncShardTrees, SyncTransactions},
        },
        witness::SHARD_HEIGHT,
    };

    type SaplingTree = ShardTree<
        shardtree::store::memory::MemoryShardStore<sapling_crypto::Node, BlockHeight>,
        { sapling_crypto::NOTE_COMMITMENT_TREE_DEPTH },
        { SHARD_HEIGHT },
    >;

    const fn h(height: u32) -> BlockHeight {
        BlockHeight::from_u32(height)
    }
    fn leaf_position() -> Position {
        Position::from(0)
    }

    /// True when the leaf at `position` still carries the MARKED retention flag.
    fn leaf_is_marked(tree: &SaplingTree, position: Position) -> bool {
        tree.store()
            .get_shard(SaplingTree::subtree_addr(position))
            .expect("infallible")
            .and_then(|shard| shard.value_at_position(position).map(|(_, flags)| *flags))
            .is_some_and(|flags| flags.contains(RetentionFlags::MARKED))
    }

    /// True when some checkpoint records `position` in its `marks_removed` set.
    fn removal_is_recorded(tree: &SaplingTree, position: Position) -> bool {
        let mut recorded = false;
        let count = tree.store().checkpoint_count().expect("infallible");
        tree.store()
            .for_each_checkpoint(count, |_, checkpoint| {
                recorded |= checkpoint.marks_removed().contains(&position);
                Ok(())
            })
            .expect("infallible");
        recorded
    }

    /// A tree holding one marked leaf at position 0 checkpointed at `marked_at`,
    /// followed by one unmarked leaf checkpointed at each of `later_checkpoints`.
    fn tree_with_marked_leaf(
        marked_at: BlockHeight,
        later_checkpoints: &[BlockHeight],
    ) -> SaplingTree {
        let mut tree = empty_shard_tree();
        tree.append(
            sapling_crypto::Node::from_scalar(jubjub::Base::from(1u64)),
            Retention::Checkpoint {
                id: marked_at,
                marking: Marking::Marked,
            },
        )
        .expect("append");
        for (i, height) in later_checkpoints.iter().enumerate() {
            tree.append(
                sapling_crypto::Node::from_scalar(jubjub::Base::from(2 + i as u64)),
                Retention::Checkpoint {
                    id: *height,
                    marking: Marking::None,
                },
            )
            .expect("append");
        }
        tree
    }

    /// Library fact (a): a spending height between the oldest and newest
    /// checkpoint ids with no checkpoint of its own is a silent no-op.
    #[test]
    fn library_remove_mark_without_checkpoint_at_height_is_silent_noop() {
        let mut tree = tree_with_marked_leaf(h(10), &[h(30)]);
        assert!(leaf_is_marked(&tree, leaf_position()));

        let removed = tree
            .remove_mark(leaf_position(), Some(&h(20)))
            .expect("infallible");

        assert!(
            !removed,
            "shardtree reports the no-op only through the bool"
        );
        assert!(leaf_is_marked(&tree, leaf_position()));
        assert!(!removal_is_recorded(&tree, leaf_position()));
        assert!(
            tree.witness_at_checkpoint_id(leaf_position(), &h(30))
                .expect("infallible")
                .is_some(),
            "the leaf is still witnessable, so the retention leaked"
        );
    }

    /// Library fact (b): a spending height below the oldest checkpoint id
    /// removes the mark directly, and no checkpoint can restore it on rollback.
    #[test]
    fn library_remove_mark_below_oldest_checkpoint_is_not_restored_by_rollback() {
        let mut tree = tree_with_marked_leaf(h(5), &[h(6), h(7), h(8), h(9), h(10)]);
        // Drop the zero-height and leaf checkpoints so the oldest checkpoint id is 6.
        tree.store_mut()
            .remove_checkpoint(&h(0))
            .expect("infallible");
        tree.store_mut()
            .remove_checkpoint(&h(5))
            .expect("infallible");
        assert_eq!(
            tree.store().min_checkpoint_id().expect("infallible"),
            Some(h(6))
        );
        assert!(
            tree.witness_at_checkpoint_id(leaf_position(), &h(6))
                .expect("infallible")
                .is_some()
        );

        let removed = tree
            .remove_mark(leaf_position(), Some(&h(2)))
            .expect("infallible");
        assert!(removed);
        assert!(!leaf_is_marked(&tree, leaf_position()));
        assert!(!removal_is_recorded(&tree, leaf_position()));

        tree.rollback_to_checkpoint(h(6)).expect("infallible");
        assert!(!leaf_is_marked(&tree, leaf_position()));
        assert!(
            tree.witness_at_checkpoint_id(leaf_position(), &h(6))
                .is_err(),
            "the rolled back tree cannot witness the un-spent note"
        );
    }

    const FUNDING_HEIGHT: BlockHeight = h(10);
    const SPEND_HEIGHT: BlockHeight = h(20);
    const LATER_CHECKPOINT: BlockHeight = h(30);
    const FUNDING_TXID: TxId = TxId::from_bytes([1; 32]);
    const SPENDING_TXID: TxId = TxId::from_bytes([2; 32]);
    const NOTE_NULLIFIER: sapling_crypto::Nullifier = sapling_crypto::Nullifier([42; 32]);

    fn funding_transaction() -> WalletTransaction {
        let extsk = sapling_crypto::zip32::ExtendedSpendingKey::master(&[0; 32]);
        let (_, recipient) = extsk.default_address();
        let crypto_note = sapling_crypto::Note::from_parts(
            recipient,
            NoteValue::from_raw(100_000),
            sapling_crypto::Rseed::AfterZip212([0; 32]),
        );
        let note = WalletNote::new_for_test(
            OutputId::new(FUNDING_TXID, 0),
            zip32::AccountId::ZERO,
            zip32::Scope::External,
            crypto_note,
            Memo::Empty,
            Some(leaf_position()),
        )
        .with_nullifier_for_test(NOTE_NULLIFIER);

        WalletTransaction::new_for_test(FUNDING_TXID, ConfirmationStatus::Confirmed(FUNDING_HEIGHT))
            .with_sapling_notes_for_test(vec![note])
    }

    /// Crate path: `update_spent_notes` with `remove_marks = true` on a tree
    /// whose checkpoints bracket the spending height without one at that
    /// height. The spend is recorded on the note, but the retention for the
    /// spent note is neither cleared nor scheduled for clearing, and the
    /// `Ok(false)` that said so was dropped by
    /// `remove_mark(...).expect("infallible")` in this module.
    #[test]
    fn update_spent_notes_drops_remove_mark_outcome() {
        let mut wallet_transactions = HashMap::new();
        wallet_transactions.insert(FUNDING_TXID, funding_transaction());
        wallet_transactions.insert(
            SPENDING_TXID,
            WalletTransaction::new_for_test(
                SPENDING_TXID,
                ConfirmationStatus::Confirmed(SPEND_HEIGHT),
            ),
        );
        let mut nullifier_map = NullifierMap::new();
        nullifier_map.sapling.insert(
            NOTE_NULLIFIER,
            ScanTarget {
                block_height: SPEND_HEIGHT,
                txid: SPENDING_TXID,
                narrow_scan_area: false,
            },
        );
        let mut shard_trees = ShardTrees::new();
        shard_trees.sapling = tree_with_marked_leaf(FUNDING_HEIGHT, &[LATER_CHECKPOINT]);
        let mut wallet = MockWalletBuilder::new()
            .wallet_transactions(wallet_transactions)
            .nullifier_map(nullifier_map)
            .shard_trees(shard_trees)
            .create_mock_wallet();

        let (sapling_nullifiers, orchard_nullifiers, ironwood_nullifiers) =
            spend::collect_derived_nullifiers(wallet.get_wallet_transactions().unwrap());
        let (sapling_targets, orchard_targets, ironwood_targets) = spend::detect_shielded_spends(
            wallet.get_nullifiers_mut().unwrap(),
            sapling_nullifiers,
            orchard_nullifiers,
            ironwood_nullifiers,
        );
        spend::update_spent_notes(
            &mut wallet,
            sapling_targets,
            orchard_targets,
            ironwood_targets,
            true,
        )
        .unwrap();

        let spending = wallet
            .get_wallet_transactions()
            .unwrap()
            .get(&FUNDING_TXID)
            .unwrap()
            .sapling_notes()
            .first()
            .unwrap()
            .spending_transaction;
        assert_eq!(
            spending,
            Some(SPENDING_TXID),
            "the spend itself is recorded"
        );

        let tree = &wallet.get_shard_trees_mut().unwrap().sapling;
        // Intended invariant: a confirmed spend with `remove_marks = true`
        // either clears the mark or records its removal in a checkpoint.
        assert!(
            !leaf_is_marked(tree, leaf_position()) || removal_is_recorded(tree, leaf_position()),
            "spent note at position 0 is still marked and no checkpoint records \
             the removal: remove_mark returned Ok(false) and the spend path ignored it"
        );
    }
}

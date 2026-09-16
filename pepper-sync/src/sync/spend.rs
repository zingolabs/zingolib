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

/// Helper function for handling spend detection and the spend status of notes.
///
/// Detects if any derived nullifiers of notes in the wallet's transactions match a nullifier in the wallet's nullifier map.
/// If a spend is detected, the nullifier is removed from the nullifier map and added to the map of spend scan targets.
/// The spend scan targets are used to set the surrounding shard block ranges to be prioritised for scanning and then to
/// fetch and scan the transactions with detected spends in the case that they evaded trial decryption.
/// Finally, all notes that were detected as spent are updated with the located spending transaction.
///
/// `additional_nullifier_map` is useful for also detecting spends for nullifiers that are not being mapped to the
/// wallet's main nullifier map.
pub(super) async fn update_shielded_spends<P, W>(
    consensus_parameters: &P,
    wallet: &mut W,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    scanned_blocks: &BTreeMap<BlockHeight, WalletBlock>,
    additional_nullifier_map: Option<&mut NullifierMap>,
) -> Result<(), SyncError<W::Error>>
where
    P: consensus::Parameters,
    W: SyncBlocks + SyncTransactions + SyncNullifiers + SyncShardTrees,
{
    let (sapling_derived_nullifiers, orchard_derived_nullifiers, ironwood_derived_nullifiers) =
        collect_derived_nullifiers(
            wallet
                .get_wallet_transactions()
                .map_err(SyncError::WalletError)?,
        );

    let (
        mut sapling_spend_scan_targets,
        mut orchard_spend_scan_targets,
        mut ironwood_spend_scan_targets,
    ) = detect_shielded_spends(
        wallet
            .get_nullifiers_mut()
            .map_err(SyncError::WalletError)?,
        sapling_derived_nullifiers.clone(),
        orchard_derived_nullifiers.clone(),
        ironwood_derived_nullifiers.clone(),
    );
    if let Some(nullifier_map) = additional_nullifier_map {
        let (
            mut additional_sapling_spend_scan_targets,
            mut additional_orchard_spend_scan_targets,
            mut additional_ironwood_spend_scan_targets,
        ) = detect_shielded_spends(
            nullifier_map,
            sapling_derived_nullifiers,
            orchard_derived_nullifiers,
            ironwood_derived_nullifiers,
        );
        sapling_spend_scan_targets.append(&mut additional_sapling_spend_scan_targets);
        orchard_spend_scan_targets.append(&mut additional_orchard_spend_scan_targets);
        ironwood_spend_scan_targets.append(&mut additional_ironwood_spend_scan_targets);
    }

    let sync_state = wallet
        .get_sync_state_mut()
        .map_err(SyncError::WalletError)?;
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

    // in the edge case where a spending transaction received no change, scan the transactions that evaded trial decryption
    scan_spending_transactions(
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
    )
    .await?;

    update_spent_notes(
        wallet,
        sapling_spend_scan_targets,
        orchard_spend_scan_targets,
        ironwood_spend_scan_targets,
        true,
    )
    .map_err(SyncError::WalletError)?;

    Ok(())
}

/// For each scan target, fetch the spending transaction and then scan and append to the wallet transactions.
///
/// This is only intended to be used for transactions that do not contain any incoming notes and therefore evaded
/// trial decryption.
/// For targetted scanning of transactions, scan targets should be added to the wallet using [`crate::add_scan_targets`] and
/// the `FoundNote` priorities will be automatically set for scan prioritisation. Transactions with incoming notes
/// are required to be scanned in the context of a scan task to correctly derive the nullifiers and positions for
/// spending.
async fn scan_spending_transactions<L, P, W>(
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    consensus_parameters: &P,
    wallet: &mut W,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    scan_targets: L,
    scanned_blocks: &BTreeMap<BlockHeight, WalletBlock>,
) -> Result<(), SyncError<W::Error>>
where
    L: Iterator<Item = ScanTarget>,
    P: consensus::Parameters,
    W: SyncBlocks + SyncTransactions + SyncNullifiers,
{
    let wallet_transactions = wallet
        .get_wallet_transactions()
        .map_err(SyncError::WalletError)?;
    let confirmed_wallet_txids = wallet_transactions
        .iter()
        .filter(|(_, transaction)| transaction.status().is_confirmed())
        .map(|(txids, _)| txids)
        .copied()
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

    wallet
        .extend_wallet_transactions(spending_transactions)
        .map_err(SyncError::WalletError)
}

/// Collects the derived nullifiers from each note in the wallet
pub(super) fn collect_derived_nullifiers(
    wallet_transactions: &HashMap<TxId, WalletTransaction>,
) -> (
    Vec<sapling_crypto::Nullifier>,
    Vec<orchard::note::Nullifier>,
    Vec<orchard::note::Nullifier>,
) {
    let sapling_nullifiers = wallet_transactions
        .values()
        .flat_map(super::super::wallet::WalletTransaction::sapling_notes)
        .filter_map(|note| note.nullifier)
        .collect::<Vec<_>>();
    let orchard_nullifiers = wallet_transactions
        .values()
        .flat_map(super::super::wallet::WalletTransaction::orchard_notes)
        .filter_map(|note| note.nullifier)
        .collect::<Vec<_>>();
    let ironwood_nullifiers = wallet_transactions
        .values()
        .flat_map(super::super::wallet::WalletTransaction::ironwood_notes)
        .filter_map(|note| note.nullifier)
        .collect::<Vec<_>>();

    (sapling_nullifiers, orchard_nullifiers, ironwood_nullifiers)
}

/// Check if any wallet note's derived nullifiers match a nullifier in the `nullifier_map`.
pub(super) fn detect_shielded_spends(
    nullifier_map: &mut NullifierMap,
    sapling_derived_nullifiers: Vec<sapling_crypto::Nullifier>,
    orchard_derived_nullifiers: Vec<orchard::note::Nullifier>,
    ironwood_derived_nullifiers: Vec<orchard::note::Nullifier>,
) -> (
    BTreeMap<sapling_crypto::Nullifier, ScanTarget>,
    BTreeMap<orchard::note::Nullifier, ScanTarget>,
    BTreeMap<orchard::note::Nullifier, ScanTarget>,
) {
    let sapling_spend_scan_targets = sapling_derived_nullifiers
        .iter()
        .filter_map(|nf| nullifier_map.sapling.remove_entry(nf))
        .collect();
    let orchard_spend_scan_targets = orchard_derived_nullifiers
        .iter()
        .filter_map(|nf| nullifier_map.orchard.remove_entry(nf))
        .collect();
    let ironwood_spend_scan_targets = ironwood_derived_nullifiers
        .iter()
        .filter_map(|nf| nullifier_map.ironwood.remove_entry(nf))
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

/// Helper function for handling spend detection and the spend status of coins.
///
/// Locates any output ids of coins in the wallet's transactions which match an output id in the wallet's outpoint map.
/// If a spend is detected, the output id is removed from the outpoint map and added to the map of spend scan targets.
/// Finally, all coins that were detected as spent are updated with the located spending transaction.
pub(super) fn update_transparent_spends<W>(
    wallet: &mut W,
    additional_outpoint_map: Option<&mut BTreeMap<OutputId, ScanTarget>>,
) -> Result<(), W::Error>
where
    W: SyncBlocks + SyncTransactions + SyncOutPoints,
{
    let transparent_output_ids = collect_transparent_output_ids(wallet.get_wallet_transactions()?);

    let mut transparent_spend_scan_targets =
        detect_transparent_spends(wallet.get_outpoints_mut()?, transparent_output_ids.clone());
    if let Some(outpoint_map) = additional_outpoint_map {
        let mut additional_transparent_spend_scan_targets =
            detect_transparent_spends(outpoint_map, transparent_output_ids);
        transparent_spend_scan_targets.append(&mut additional_transparent_spend_scan_targets);
    }

    update_spent_coins(
        wallet.get_wallet_transactions_mut()?,
        transparent_spend_scan_targets,
    );

    Ok(())
}

/// Collects the output ids from each coin in the wallet
pub(super) fn collect_transparent_output_ids(
    wallet_transactions: &HashMap<TxId, WalletTransaction>,
) -> Vec<OutputId> {
    wallet_transactions
        .values()
        .flat_map(super::super::wallet::WalletTransaction::transparent_coins)
        .map(|coin| coin.output_id)
        .collect()
}

/// Check if any wallet coin's output id match an outpoint in the `outpoint_map`.
pub(super) fn detect_transparent_spends(
    outpoint_map: &mut BTreeMap<OutputId, ScanTarget>,
    transparent_output_ids: Vec<OutputId>,
) -> BTreeMap<OutputId, ScanTarget> {
    transparent_output_ids
        .iter()
        .filter_map(|output_id| outpoint_map.remove_entry(output_id))
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

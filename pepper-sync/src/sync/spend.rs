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

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use shardtree::store::ShardStore;
    use zcash_protocol::consensus::BlockHeight;

    use crate::{
        mocks::{MockWallet, MockWalletBuilder, MockWalletError},
        wallet::{
            ShardTrees, SyncState, WalletTransaction,
            traits::{SyncShardTrees, SyncTransactions, SyncWallet},
        },
    };

    /// Wraps a `MockWallet` and fails `get_wallet_transactions_mut`, which is the first
    /// fallible call after `update_spent_notes` has taken the shard trees out of the wallet.
    struct FailingTransactionsWallet {
        inner: MockWallet,
    }

    impl SyncWallet for FailingTransactionsWallet {
        type Error = MockWalletError;

        fn get_birthday(&self) -> Result<BlockHeight, Self::Error> {
            self.inner.get_birthday()
        }

        fn get_sync_state(&self) -> Result<&SyncState, Self::Error> {
            self.inner.get_sync_state()
        }

        fn get_sync_state_mut(&mut self) -> Result<&mut SyncState, Self::Error> {
            self.inner.get_sync_state_mut()
        }

        fn get_unified_full_viewing_keys(
            &self,
        ) -> Result<HashMap<zip32::AccountId, zcash_keys::keys::UnifiedFullViewingKey>, Self::Error>
        {
            self.inner.get_unified_full_viewing_keys()
        }

        fn add_orchard_address(
            &mut self,
            account_id: zip32::AccountId,
            address: orchard::Address,
            diversifier_index: zip32::DiversifierIndex,
        ) -> Result<(), Self::Error> {
            self.inner
                .add_orchard_address(account_id, address, diversifier_index)
        }

        fn add_sapling_address(
            &mut self,
            account_id: zip32::AccountId,
            address: sapling_crypto::PaymentAddress,
            diversifier_index: zip32::DiversifierIndex,
        ) -> Result<(), Self::Error> {
            self.inner
                .add_sapling_address(account_id, address, diversifier_index)
        }

        fn get_transparent_addresses(
            &self,
        ) -> Result<&BTreeMap<crate::keys::transparent::TransparentAddressId, String>, Self::Error>
        {
            self.inner.get_transparent_addresses()
        }

        fn get_transparent_addresses_mut(
            &mut self,
        ) -> Result<
            &mut BTreeMap<crate::keys::transparent::TransparentAddressId, String>,
            Self::Error,
        > {
            self.inner.get_transparent_addresses_mut()
        }
    }

    impl SyncTransactions for FailingTransactionsWallet {
        fn get_wallet_transactions(
            &self,
        ) -> Result<&HashMap<zcash_protocol::TxId, WalletTransaction>, Self::Error> {
            self.inner.get_wallet_transactions()
        }

        fn get_wallet_transactions_mut(
            &mut self,
        ) -> Result<&mut HashMap<zcash_protocol::TxId, WalletTransaction>, Self::Error> {
            Err(MockWalletError::AnErrorVariant(
                "wallet transactions unavailable".to_string(),
            ))
        }
    }

    impl SyncShardTrees for FailingTransactionsWallet {
        fn get_shard_trees(&self) -> Result<&ShardTrees, Self::Error> {
            self.inner.get_shard_trees()
        }

        fn get_shard_trees_mut(&mut self) -> Result<&mut ShardTrees, Self::Error> {
            self.inner.get_shard_trees_mut()
        }
    }

    // REPRO: claim `mem-take-leaves-empty-shard-trees`. `update_spent_notes` moves the wallet's
    // shard trees out with `std::mem::take` and only writes them back on the happy path, so any
    // error raised in between leaves the wallet holding `ShardTrees::default()`.
    #[test]
    fn failed_spent_note_update_keeps_wallet_shard_trees() {
        let checkpoint = BlockHeight::from_u32(5);
        let mut shard_trees = ShardTrees::new();
        assert!(shard_trees.orchard.checkpoint(checkpoint).unwrap());

        let mut wallet = FailingTransactionsWallet {
            inner: MockWalletBuilder::new()
                .shard_trees(shard_trees)
                .create_mock_wallet(),
        };

        let result = super::update_spent_notes(
            &mut wallet,
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            true,
        );
        assert!(result.is_err());

        let newest_checkpoint = wallet
            .get_shard_trees_mut()
            .unwrap()
            .orchard
            .store()
            .max_checkpoint_id()
            .unwrap();
        assert_eq!(
            newest_checkpoint,
            Some(checkpoint),
            "shard trees must survive a failed spent note update"
        );
    }
}

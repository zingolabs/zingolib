use std::{
    array::TryFromSliceError,
    collections::{BTreeMap, BTreeSet, HashMap},
};

use orchard::tree::MerkleHashOrchard;
use task::ScanTask;
use tokio::sync::mpsc;

use incrementalmerkletree::Position;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::{self, BlockHeight};
use zingo_netutils::lightwallet_protocol::{CompactBlock, CompactTx};
use zip32::AccountId;

use crate::{
    client::FetchRequest,
    error::{ScanError, ServerError},
    keys::transparent::TransparentAddressId,
    sync::ScanPriority,
    utils::{block, transaction},
    wallet::{NullifierMap, OutputId, ScanTarget, WalletBlock, WalletTransaction},
    witness::{self, LocatedTreeData, WitnessData},
};

use self::{compact_blocks::scan_compact_blocks, transactions::scan_transactions};

pub(crate) mod compact_blocks;
pub(crate) mod task;
pub(crate) mod transactions;

#[derive(Debug, Clone)]
struct InitialScanData {
    start_seam_block: Option<WalletBlock>,
    end_seam_block: Option<WalletBlock>,
    sapling_initial_tree_size: u32,
    orchard_initial_tree_size: u32,
    ironwood_initial_tree_size: u32,
}

impl InitialScanData {
    async fn new<P>(
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        consensus_parameters: &P,
        first_block: &CompactBlock,
        start_seam_block: Option<WalletBlock>,
        end_seam_block: Option<WalletBlock>,
    ) -> Result<Self, ServerError>
    where
        P: consensus::Parameters + Sync + Send + 'static,
    {
        let (sapling_initial_tree_size, orchard_initial_tree_size, ironwood_initial_tree_size) =
            if let Some(prev) = &start_seam_block {
                (
                    prev.tree_bounds().sapling_final_tree_size,
                    prev.tree_bounds().orchard_final_tree_size,
                    prev.tree_bounds().ironwood_final_tree_size,
                )
            } else {
                let tree_bounds = compact_blocks::calculate_block_tree_bounds(
                    consensus_parameters,
                    fetch_request_sender,
                    first_block,
                )
                .await?;

                (
                    tree_bounds.sapling_initial_tree_size,
                    tree_bounds.orchard_initial_tree_size,
                    tree_bounds.ironwood_initial_tree_size,
                )
            };

        Ok(InitialScanData {
            start_seam_block,
            end_seam_block,
            sapling_initial_tree_size,
            orchard_initial_tree_size,
            ironwood_initial_tree_size,
        })
    }
}

struct ScanData {
    nullifiers: NullifierMap,
    outpoints: BTreeMap<OutputId, ScanTarget>,
    wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    decrypted_scan_targets: BTreeSet<ScanTarget>,
    decrypted_note_data: DecryptedNoteData,
    witness_data: WitnessData,
    new_transparent_inuse_addresses: HashMap<String, TransparentAddressId>,
    updated_transparent_gap_addresses: HashMap<String, TransparentAddressId>,
}

#[derive(Debug)]
pub(crate) struct ScanResults {
    pub(crate) nullifiers: NullifierMap,
    pub(crate) outpoints: BTreeMap<OutputId, ScanTarget>,
    pub(crate) scanned_blocks: BTreeMap<BlockHeight, WalletBlock>,
    pub(crate) wallet_transactions: HashMap<TxId, WalletTransaction>,
    pub(crate) sapling_located_trees: Vec<LocatedTreeData<sapling_crypto::Node>>,
    pub(crate) orchard_located_trees: Vec<LocatedTreeData<MerkleHashOrchard>>,
    pub(crate) ironwood_located_trees: Vec<LocatedTreeData<MerkleHashOrchard>>,
    pub(crate) new_transparent_inuse_addresses: HashMap<String, TransparentAddressId>,
    pub(crate) updated_transparent_gap_addresses: HashMap<String, TransparentAddressId>,
}

pub(crate) struct DecryptedNoteData {
    sapling_nullifiers_and_positions: HashMap<OutputId, (sapling_crypto::Nullifier, Position)>,
    orchard_nullifiers_and_positions: HashMap<OutputId, (orchard::note::Nullifier, Position)>,
    ironwood_nullifiers_and_positions: HashMap<OutputId, (orchard::note::Nullifier, Position)>,
}

impl DecryptedNoteData {
    pub(crate) fn new() -> Self {
        DecryptedNoteData {
            sapling_nullifiers_and_positions: HashMap::new(),
            orchard_nullifiers_and_positions: HashMap::new(),
            ironwood_nullifiers_and_positions: HashMap::new(),
        }
    }
}

/// Scans a given range and returns all data relevant to the specified keys.
///
/// `start_seam_block` and `end_seam_block` are the blocks adjacent to the `scan_range` for verification of continuity.
/// `scan_targets` are the block height and txid of transactions in the `scan_range` that are known to be relevant to the
/// wallet and are appended to during scanning if trial decryption succeeds. If there are no known relevant transctions
/// then `scan_targets` will start empty.
pub(crate) async fn scan<P>(
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    consensus_parameters: &P,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    scan_task: ScanTask,
    max_outputs: usize,
    transparent_gap_limit: u32,
) -> Result<ScanResults, ScanError>
where
    P: consensus::Parameters + Sync + Send + 'static,
{
    let ScanTask {
        compact_blocks,
        scan_range,
        start_seam_block,
        end_seam_block,
        mut scan_targets,
        mut transparent_inuse_addresses,
        transparent_gap_addresses,
    } = scan_task;

    if compact_blocks
        .first()
        .expect("compacts blocks should not be empty")
        .height
        != u64::from(scan_range.block_range().start)
        || compact_blocks
            .last()
            .expect("compacts blocks should not be empty")
            .height
            != u64::from(scan_range.block_range().end - 1)
    {
        panic!("compact blocks do not match scan range!")
    }

    if scan_range.priority() == ScanPriority::ScannedWithoutMapping {
        let mut nullifiers = NullifierMap::new();
        for block in &compact_blocks {
            for transaction in &block.vtx {
                collect_nullifiers_compact(
                    &mut nullifiers,
                    block::get_compact_height(block),
                    transaction,
                )?;
            }
        }

        return Ok(ScanResults {
            nullifiers,
            outpoints: BTreeMap::new(),
            scanned_blocks: BTreeMap::new(),
            wallet_transactions: HashMap::new(),
            sapling_located_trees: Vec::new(),
            orchard_located_trees: Vec::new(),
            ironwood_located_trees: Vec::new(),
            new_transparent_inuse_addresses: HashMap::new(),
            updated_transparent_gap_addresses: transparent_gap_addresses,
        });
    }

    let initial_scan_data = InitialScanData::new(
        fetch_request_sender.clone(),
        consensus_parameters,
        compact_blocks
            .first()
            .expect("compacts blocks should not be empty"),
        start_seam_block,
        end_seam_block,
    )
    .await?;

    let consensus_parameters_clone = consensus_parameters.clone();
    let ufvks_clone = ufvks.clone();
    let initial_scan_data_clone = initial_scan_data.clone();
    let transparent_inuse_addresses_clone = transparent_inuse_addresses.clone();
    let scan_data = tokio::task::spawn_blocking(move || {
        scan_compact_blocks(
            compact_blocks,
            &consensus_parameters_clone,
            &ufvks_clone,
            initial_scan_data_clone,
            max_outputs / 8,
            transparent_inuse_addresses_clone,
            transparent_gap_addresses,
            transparent_gap_limit,
        )
    })
    .await
    .expect("task panicked")?;

    let ScanData {
        nullifiers,
        mut outpoints,
        wallet_blocks,
        mut decrypted_scan_targets,
        decrypted_note_data,
        witness_data,
        new_transparent_inuse_addresses,
        updated_transparent_gap_addresses,
    } = scan_data;
    scan_targets.append(&mut decrypted_scan_targets);
    transparent_inuse_addresses.extend(new_transparent_inuse_addresses.clone());

    let wallet_transactions = scan_transactions(
        fetch_request_sender,
        consensus_parameters,
        ufvks,
        scan_targets,
        decrypted_note_data,
        &wallet_blocks,
        &mut outpoints,
        transparent_inuse_addresses,
    )
    .await?;

    let WitnessData {
        sapling_initial_position,
        orchard_initial_position,
        ironwood_initial_position,
        sapling_leaves_and_retentions,
        orchard_leaves_and_retentions,
        ironwood_leaves_and_retentions,
    } = witness_data;

    let (sapling_located_trees, orchard_located_trees, ironwood_located_trees) =
        tokio::task::spawn_blocking(move || {
            (
                witness::build_located_trees(
                    sapling_initial_position,
                    sapling_leaves_and_retentions,
                    max_outputs / 8,
                ),
                witness::build_located_trees(
                    orchard_initial_position,
                    orchard_leaves_and_retentions,
                    max_outputs / 8,
                ),
                witness::build_located_trees(
                    ironwood_initial_position,
                    ironwood_leaves_and_retentions,
                    max_outputs / 8,
                ),
            )
        })
        .await
        .expect("task panicked");

    Ok(ScanResults {
        nullifiers,
        outpoints,
        scanned_blocks: wallet_blocks,
        wallet_transactions,
        sapling_located_trees,
        orchard_located_trees,
        ironwood_located_trees,
        new_transparent_inuse_addresses,
        updated_transparent_gap_addresses,
    })
}

/// Converts the nullifiers from a compact transaction and adds them to the nullifier map
fn collect_nullifiers_compact(
    nullifier_map: &mut NullifierMap,
    block_height: BlockHeight,
    transaction: &CompactTx,
) -> Result<(), ScanError> {
    let txid = transaction::get_compact_txid(transaction);

    transaction
        .spends
        .iter()
        .map(|spend| sapling_crypto::Nullifier::from_slice(spend.nf.as_slice()))
        .collect::<Result<Vec<sapling_crypto::Nullifier>, TryFromSliceError>>()?
        .into_iter()
        .for_each(|nullifier| {
            nullifier_map.sapling.insert(
                nullifier,
                ScanTarget {
                    block_height,
                    txid,
                    narrow_scan_area: false,
                },
            );
        });
    transaction
        .actions
        .iter()
        .map(|action| {
            orchard::note::Nullifier::from_bytes(
                action.nullifier.as_slice().try_into().map_err(|_| {
                    ScanError::InvalidOrchardNullifierLength(action.nullifier.len())
                })?,
            )
            .into_option()
            .ok_or(ScanError::InvalidOrchardNullifier)
        })
        .collect::<Result<Vec<orchard::note::Nullifier>, ScanError>>()?
        .into_iter()
        .for_each(|nullifier| {
            nullifier_map.orchard.insert(
                nullifier,
                ScanTarget {
                    block_height,
                    txid,
                    narrow_scan_area: false,
                },
            );
        });
    transaction
        .ironwood_actions
        .iter()
        .map(|action| {
            orchard::note::Nullifier::from_bytes(
                action.nullifier.as_slice().try_into().map_err(|_| {
                    ScanError::InvalidOrchardNullifierLength(action.nullifier.len())
                })?,
            )
            .into_option()
            .ok_or(ScanError::InvalidOrchardNullifier)
        })
        .collect::<Result<Vec<orchard::note::Nullifier>, ScanError>>()?
        .into_iter()
        .for_each(|nullifier| {
            nullifier_map.ironwood.insert(
                nullifier,
                ScanTarget {
                    block_height,
                    txid,
                    narrow_scan_area: false,
                },
            );
        });
    Ok(())
}

/// Adds the outpoints from a compact transaction to the outpoint map.
fn collect_outpoints_compact(
    outpoint_map: &mut BTreeMap<OutputId, ScanTarget>,
    block_height: BlockHeight,
    transaction: &CompactTx,
) {
    let txid = transaction::get_compact_txid(transaction);

    transaction.vin.iter().for_each(|outpoint| {
        let mut txid_bytes = [0u8; 32];
        txid_bytes.copy_from_slice(&outpoint.prevout_txid);
        let prevout_txid = TxId::from_bytes(txid_bytes);
        outpoint_map.insert(
            OutputId::new(prevout_txid, outpoint.prevout_index),
            ScanTarget {
                block_height,
                txid,
                narrow_scan_area: true,
            },
        );
    });
}

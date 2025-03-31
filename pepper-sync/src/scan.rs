use std::collections::{BTreeMap, BTreeSet, HashMap};

use orchard::tree::MerkleHashOrchard;
use task::ScanTask;
use tokio::sync::mpsc;

use incrementalmerkletree::Position;
use zcash_client_backend::proto::compact_formats::CompactBlock;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::{
    consensus::{self, BlockHeight},
    transaction::TxId,
    zip32::AccountId,
};

use crate::{
    client::FetchRequest,
    wallet::{Locator, NullifierMap, OutputId, WalletBlock, WalletTransaction},
    witness::{self, LocatedTreeData, WitnessData},
};

use self::{
    compact_blocks::scan_compact_blocks, error::ScanError, transactions::scan_transactions,
};

pub(crate) mod compact_blocks;
pub mod error;
pub(crate) mod task;
pub(crate) mod transactions;

struct InitialScanData {
    start_seam_block: Option<WalletBlock>,
    end_seam_block: Option<WalletBlock>,
    sapling_initial_tree_size: u32,
    orchard_initial_tree_size: u32,
}

impl InitialScanData {
    async fn new<P>(
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        consensus_parameters: &P,
        first_block: &CompactBlock,
        start_seam_block: Option<WalletBlock>,
        end_seam_block: Option<WalletBlock>,
    ) -> Result<Self, ()>
    where
        P: consensus::Parameters + Sync + Send + 'static,
    {
        let (sapling_initial_tree_size, orchard_initial_tree_size) =
            if let Some(prev) = &start_seam_block {
                (
                    prev.tree_bounds().sapling_final_tree_size,
                    prev.tree_bounds().orchard_final_tree_size,
                )
            } else {
                let tree_bounds = compact_blocks::calculate_block_tree_bounds(
                    consensus_parameters,
                    fetch_request_sender,
                    first_block,
                )
                .await;

                (
                    tree_bounds.sapling_initial_tree_size,
                    tree_bounds.orchard_initial_tree_size,
                )
            };

        Ok(InitialScanData {
            start_seam_block,
            end_seam_block,
            sapling_initial_tree_size,
            orchard_initial_tree_size,
        })
    }
}

struct ScanData {
    nullifiers: NullifierMap,
    wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    decrypted_locators: BTreeSet<Locator>,
    decrypted_note_data: DecryptedNoteData,
    witness_data: WitnessData,
}

pub(crate) struct ScanResults {
    pub(crate) nullifiers: NullifierMap,
    pub(crate) outpoints: BTreeMap<OutputId, Locator>,
    pub(crate) wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    pub(crate) wallet_transactions: HashMap<TxId, WalletTransaction>,
    pub(crate) sapling_located_trees: Vec<LocatedTreeData<sapling_crypto::Node>>,
    pub(crate) orchard_located_trees: Vec<LocatedTreeData<MerkleHashOrchard>>,
}

pub(crate) struct DecryptedNoteData {
    sapling_nullifiers_and_positions: HashMap<OutputId, (sapling_crypto::Nullifier, Position)>,
    orchard_nullifiers_and_positions: HashMap<OutputId, (orchard::note::Nullifier, Position)>,
}

impl DecryptedNoteData {
    pub(crate) fn new() -> Self {
        DecryptedNoteData {
            sapling_nullifiers_and_positions: HashMap::new(),
            orchard_nullifiers_and_positions: HashMap::new(),
        }
    }
}

/// Scans a given range and returns all data relevant to the specified keys.
///
/// `start_seam_block` and `end_seam_block` are the blocks adjacent to the `scan_range` for verification of continuity.
/// `locators` are the block height and txid of transactions in the `scan_range` that are known to be relevant to the
/// wallet and are appended to during scanning if trial decryption succeeds. If there are no known relevant transctions
/// then `locators` will start empty.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn scan<P>(
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    consensus_parameters: &P,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    scan_task: ScanTask,
) -> Result<ScanResults, ScanError>
where
    P: consensus::Parameters + Sync + Send + 'static,
{
    let ScanTask {
        compact_blocks,
        scan_range,
        start_seam_block,
        end_seam_block,
        mut locators,
        transparent_addresses,
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

    let initial_scan_data = InitialScanData::new(
        fetch_request_sender.clone(),
        consensus_parameters,
        compact_blocks
            .first()
            .expect("compacts blocks should not be empty"),
        start_seam_block,
        end_seam_block,
    )
    .await
    .unwrap();

    let consensus_parameters_clone = consensus_parameters.clone();
    let ufvks_clone = ufvks.clone();
    let scan_data = tokio::task::spawn_blocking(move || {
        scan_compact_blocks(
            compact_blocks,
            &consensus_parameters_clone,
            &ufvks_clone,
            initial_scan_data,
        )
    })
    .await
    .unwrap()?;

    let ScanData {
        nullifiers,
        wallet_blocks,
        mut decrypted_locators,
        decrypted_note_data,
        witness_data,
    } = scan_data;

    locators.append(&mut decrypted_locators);

    let mut outpoints = BTreeMap::new();
    let wallet_transactions = scan_transactions(
        fetch_request_sender,
        consensus_parameters,
        ufvks,
        locators,
        decrypted_note_data,
        &wallet_blocks,
        &mut outpoints,
        transparent_addresses,
    )
    .await
    .unwrap();

    let WitnessData {
        sapling_initial_position,
        orchard_initial_position,
        sapling_leaves_and_retentions,
        orchard_leaves_and_retentions,
    } = witness_data;

    let (sapling_located_trees, orchard_located_trees) = tokio::task::spawn_blocking(move || {
        (
            witness::build_located_trees(sapling_initial_position, sapling_leaves_and_retentions)
                .unwrap(),
            witness::build_located_trees(orchard_initial_position, orchard_leaves_and_retentions)
                .unwrap(),
        )
    })
    .await
    .unwrap();

    Ok(ScanResults {
        nullifiers,
        outpoints,
        wallet_blocks,
        wallet_transactions,
        sapling_located_trees,
        orchard_located_trees,
    })
}

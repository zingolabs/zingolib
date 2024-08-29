use std::{
    cmp,
    collections::{BTreeMap, HashMap, HashSet},
};

use incrementalmerkletree::Position;
use tokio::sync::mpsc;
use zcash_client_backend::{data_api::scanning::ScanRange, proto::compact_formats::CompactBlock};
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::{
    consensus::{BlockHeight, NetworkUpgrade, Parameters},
    transaction::TxId,
    zip32::AccountId,
};

use crate::{
    client::{self, FetchRequest},
    primitives::{NullifierMap, OutputId, WalletBlock, WalletTransaction},
    witness::ShardTreeData,
};

use self::{
    compact_blocks::scan_compact_blocks, error::ScanError, transactions::scan_transactions,
};

mod compact_blocks;
pub mod error;
pub(crate) mod task;
pub(crate) mod transactions;

struct InitialScanData {
    previous_block: Option<WalletBlock>,
    sapling_initial_tree_size: u32,
    orchard_initial_tree_size: u32,
}

impl InitialScanData {
    async fn new<P>(
        fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
        parameters: &P,
        first_block: &CompactBlock,
        previous_wallet_block: Option<WalletBlock>,
    ) -> Result<Self, ()>
    where
        P: Parameters + Sync + Send + 'static,
    {
        // gets initial tree size from previous block if available
        // otherwise, from first block if available
        // otherwise, fetches frontiers from server
        let (sapling_initial_tree_size, orchard_initial_tree_size) =
            if let Some(prev) = &previous_wallet_block {
                (
                    prev.sapling_commitment_tree_size(),
                    prev.orchard_commitment_tree_size(),
                )
            } else if let Some(chain_metadata) = first_block.chain_metadata.clone() {
                // calculate initial tree size by subtracting number of outputs in block from the blocks final tree size
                let sapling_output_count: u32 = first_block
                    .vtx
                    .iter()
                    .map(|tx| tx.outputs.len())
                    .sum::<usize>()
                    .try_into()
                    .expect("Sapling output count cannot exceed a u32");
                let orchard_output_count: u32 = first_block
                    .vtx
                    .iter()
                    .map(|tx| tx.actions.len())
                    .sum::<usize>()
                    .try_into()
                    .expect("Sapling output count cannot exceed a u32");

                (
                    chain_metadata
                        .sapling_commitment_tree_size
                        .checked_sub(sapling_output_count)
                        .unwrap(),
                    chain_metadata
                        .orchard_commitment_tree_size
                        .checked_sub(orchard_output_count)
                        .unwrap(),
                )
            } else {
                let sapling_activation_height = parameters
                    .activation_height(NetworkUpgrade::Sapling)
                    .expect("should have some sapling activation height");

                match first_block.height().cmp(&sapling_activation_height) {
                    cmp::Ordering::Greater => {
                        let frontiers =
                            client::get_frontiers(fetch_request_sender, first_block.height() - 1)
                                .await
                                .unwrap();
                        (
                            frontiers.final_sapling_tree().tree_size() as u32,
                            frontiers.final_orchard_tree().tree_size() as u32,
                        )
                    }
                    cmp::Ordering::Equal => (0, 0),
                    cmp::Ordering::Less => panic!("pre-sapling not supported!"),
                }
            };

        Ok(InitialScanData {
            previous_block: previous_wallet_block,
            sapling_initial_tree_size,
            orchard_initial_tree_size,
        })
    }
}

struct ScanData {
    nullifiers: NullifierMap,
    wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    relevant_txids: HashSet<TxId>,
    decrypted_note_data: DecryptedNoteData,
    shard_tree_data: ShardTreeData,
}

pub(crate) struct ScanResults {
    pub(crate) nullifiers: NullifierMap,
    pub(crate) wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    pub(crate) wallet_transactions: HashMap<TxId, WalletTransaction>,
    pub(crate) shard_tree_data: ShardTreeData,
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
/// `previous_wallet_block` is the wallet block with height [scan_range.start - 1].
pub(crate) async fn scan<P>(
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    parameters: &P,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    scan_range: ScanRange,
    previous_wallet_block: Option<WalletBlock>,
) -> Result<ScanResults, ScanError>
where
    P: Parameters + Sync + Send + 'static,
{
    let compact_blocks = client::get_compact_block_range(
        fetch_request_sender.clone(),
        scan_range.block_range().clone(),
    )
    .await
    .unwrap();

    let initial_scan_data = InitialScanData::new(
        fetch_request_sender.clone(),
        parameters,
        compact_blocks
            .first()
            .expect("compacts blocks should not be empty"),
        previous_wallet_block,
    )
    .await
    .unwrap();

    let scan_data = scan_compact_blocks(compact_blocks, parameters, ufvks, initial_scan_data)?;

    let ScanData {
        nullifiers,
        wallet_blocks,
        relevant_txids,
        decrypted_note_data,
        shard_tree_data,
    } = scan_data;

    let wallet_transactions = scan_transactions(
        fetch_request_sender,
        parameters,
        ufvks,
        relevant_txids,
        decrypted_note_data,
        &wallet_blocks,
    )
    .await
    .unwrap();

    Ok(ScanResults {
        nullifiers,
        wallet_blocks,
        wallet_transactions,
        shard_tree_data,
    })
}

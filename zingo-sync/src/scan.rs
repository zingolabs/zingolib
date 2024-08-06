use std::collections::HashSet;

use orchard::keys::Scope;
use tokio::sync::mpsc::UnboundedSender;
use zcash_client_backend::{
    data_api::scanning::ScanRange, proto::compact_formats::CompactBlock, scanning::ScanningKeys,
    PoolType, ShieldedProtocol,
};
use zcash_primitives::{
    block::BlockHash,
    consensus::{BlockHeight, Parameters},
    transaction::TxId,
    zip32::AccountId,
};

use crate::{
    client::{self, get_compact_block_range, FetchRequest},
    primitives::{OutputId, WalletCompactBlock},
};

use self::runners::BatchRunners;

pub(crate) mod runners;

struct InitialScanData<'a> {
    previous_block: Option<&'a WalletCompactBlock>,
    sapling_initial_tree_size: u32,
    orchard_initial_tree_size: u32,
}

impl InitialScanData<'_> {
    async fn new(
        fetch_request_sender: UnboundedSender<FetchRequest>,
        first_block: &CompactBlock,
    ) -> Result<Self, ()> {
        // TODO: get last block of adjacent lower scan range a.k.a. previous_block
        let previous_block: Option<&WalletCompactBlock> = None;

        // gets initial tree size from previous block if available
        // otherwise, from first block if available
        // otherwise, fetches frontiers from server
        let (sapling_initial_tree_size, orchard_initial_tree_size) =
            if let Some(prev) = previous_block {
                (
                    prev.sapling_commitment_tree_size(),
                    prev.orchard_commitment_tree_size(),
                )
            } else {
                if let Some(chain_metadata) = first_block.chain_metadata.clone() {
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
                    let frontiers =
                        client::get_frontiers(fetch_request_sender, first_block.height() - 1)
                            .await
                            .unwrap();
                    (
                        frontiers.final_sapling_tree().tree_size() as u32,
                        frontiers.final_orchard_tree().tree_size() as u32,
                    )
                }
            };

        Ok(InitialScanData {
            previous_block,
            sapling_initial_tree_size,
            orchard_initial_tree_size,
        })
    }
}

pub(crate) async fn scan<P>(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    parameters: &P,
    scanning_keys: &ScanningKeys<AccountId, (AccountId, Scope)>,
    scan_range: ScanRange,
) -> Result<(), ()>
where
    P: Parameters + Send + 'static,
{
    let compact_blocks = get_compact_block_range(
        fetch_request_sender.clone(),
        scan_range.block_range().clone(),
    )
    .await
    .unwrap();

    let initial_scan_data = InitialScanData::new(
        fetch_request_sender,
        compact_blocks
            .first()
            .expect("compacts blocks should not be empty"),
    )
    .await
    .unwrap();

    check_continuity(&compact_blocks, initial_scan_data.previous_block).unwrap();

    let (incoming_output_ids, outgoing_output_ids) =
        trial_decrypt(parameters, scanning_keys, &compact_blocks).unwrap();

    // gather the IDs of all transactions relevent to the wallet
    // the edge case of transactions that this capability created but did not receive change
    // or create outgoing data is handled when the nullifiers are added and linked
    let mut relevent_txids: HashSet<TxId> = HashSet::new();
    incoming_output_ids
        .iter()
        .chain(outgoing_output_ids.iter())
        .for_each(|output_id| {
            relevent_txids.insert(*output_id.txid());
        });

    // TODO: build shard tree, calculate nullifiers and positions for incoming outputs, map nullifiers and write compact blocks
    let mut sapling_tree_size = initial_scan_data.sapling_initial_tree_size;
    let mut orchard_tree_size = initial_scan_data.orchard_initial_tree_size;
    for block in compact_blocks {
        // let zip212_enforcement = zip212_enforcement(parameters, compact_block.height());
        for transaction in block.vtx {
            //
        }
    }

    // FIXME: panics when less than 0 for regtest or less than sapling epoch for mainnet
    // let _frontiers = get_frontiers(fetch_request_sender, scan_range.block_range().start - 1)
    //     .await
    //     .unwrap();

    // let mut sapling_nullifiers_and_positions: HashMap<
    //     OutputId,
    //     (sapling_crypto::Nullifier, Position),
    // > = HashMap::new();
    // let mut orchard_nullifiers_and_positions: HashMap<
    //     OutputId,
    //     (orchard::note::Nullifier, Position),
    // > = HashMap::new();

    Ok(())
}

fn trial_decrypt<P>(
    parameters: &P,
    scanning_keys: &ScanningKeys<AccountId, (AccountId, Scope)>,
    compact_blocks: &[CompactBlock],
) -> Result<(Vec<OutputId>, Vec<OutputId>), ()>
where
    P: Parameters + Send + 'static,
{
    let mut runners = BatchRunners::<_, (), ()>::for_keys(100, scanning_keys);
    for block in compact_blocks {
        runners.add_block(parameters, block.clone()).unwrap();
    }
    runners.flush();

    let mut incoming_output_ids: Vec<OutputId> = Vec::new();
    let outgoing_output_ids: Vec<OutputId> = Vec::new(); // TODO: add outgoing decryption
    for block in compact_blocks {
        for transaction in block.vtx.iter() {
            let decrypted_sapling_outputs = runners
                .sapling
                .collect_results(block.hash(), transaction.txid());
            decrypted_sapling_outputs
                .into_keys()
                .for_each(|(txid, output_index)| {
                    incoming_output_ids.push(OutputId::from_parts(
                        txid,
                        output_index,
                        PoolType::Shielded(ShieldedProtocol::Sapling),
                    ));
                });

            let decrypted_orchard_outputs = runners
                .orchard
                .collect_results(block.hash(), transaction.txid());
            decrypted_orchard_outputs
                .into_keys()
                .for_each(|(txid, output_index)| {
                    incoming_output_ids.push(OutputId::from_parts(
                        txid,
                        output_index,
                        PoolType::Shielded(ShieldedProtocol::Orchard),
                    ));
                });
        }
    }

    Ok((incoming_output_ids, outgoing_output_ids))
}

// checks height and hash continuity of a batch of compact blocks.
// takes the last wallet compact block of the adjacent lower scan range, if available.
fn check_continuity(
    compact_blocks: &[CompactBlock],
    previous_compact_block: Option<&WalletCompactBlock>,
) -> Result<(), ()> {
    let mut prev_height: Option<BlockHeight> = None;
    let mut prev_hash: Option<BlockHash> = None;

    if let Some(prev) = previous_compact_block {
        prev_height = Some(prev.block_height());
        prev_hash = Some(prev.block_hash());
    }

    for block in compact_blocks {
        if let Some(prev_height) = prev_height {
            if block.height() != prev_height + 1 {
                panic!("height discontinuity");
            }
        }

        if let Some(prev_hash) = prev_hash {
            if block.prev_hash() != prev_hash {
                panic!("hash discontinuity");
            }
        }

        prev_height = Some(block.height());
        prev_hash = Some(block.hash());
    }

    Ok(())
}

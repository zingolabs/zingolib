use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap, HashSet},
};

use tokio::sync::mpsc::UnboundedSender;

use incrementalmerkletree::{Position, Retention};
use orchard::{note_encryption::CompactAction, tree::MerkleHashOrchard};
use sapling_crypto::{note_encryption::CompactOutputDescription, Node};
use zcash_client_backend::{
    data_api::scanning::ScanRange,
    proto::compact_formats::{CompactBlock, CompactOrchardAction, CompactSaplingOutput, CompactTx},
};
use zcash_note_encryption::Domain;
use zcash_primitives::{
    block::BlockHash,
    consensus::{BlockHeight, NetworkUpgrade, Parameters},
    transaction::TxId,
};

use crate::{
    client::{self, get_compact_block_range, FetchRequest},
    keys::{KeyId, ScanningKeyOps, ScanningKeys},
    primitives::{NullifierMap, OutputId, WalletBlock},
    witness::ShardTreeData,
};

use self::runners::{BatchRunners, DecryptedOutput};

pub(crate) mod runners;

struct InitialScanData {
    previous_block: Option<WalletBlock>,
    sapling_initial_tree_size: u32,
    orchard_initial_tree_size: u32,
}

impl InitialScanData {
    async fn new<P>(
        fetch_request_sender: UnboundedSender<FetchRequest>,
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
                    Ordering::Greater => {
                        let frontiers =
                            client::get_frontiers(fetch_request_sender, first_block.height() - 1)
                                .await
                                .unwrap();
                        (
                            frontiers.final_sapling_tree().tree_size() as u32,
                            frontiers.final_orchard_tree().tree_size() as u32,
                        )
                    }
                    Ordering::Equal => (0, 0),
                    Ordering::Less => panic!("pre-sapling not supported!"),
                }
            };

        Ok(InitialScanData {
            previous_block: previous_wallet_block,
            sapling_initial_tree_size,
            orchard_initial_tree_size,
        })
    }
}

pub(crate) struct ScanData {
    pub(crate) nullifiers: NullifierMap,
    pub(crate) wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    pub(crate) shard_tree_data: ShardTreeData,
}

struct DecryptedNoteData {
    sapling_nullifiers_and_positions: HashMap<OutputId, (sapling_crypto::Nullifier, Position)>,
    orchard_nullifiers_and_positions: HashMap<OutputId, (orchard::note::Nullifier, Position)>,
}

impl DecryptedNoteData {
    fn new() -> Self {
        DecryptedNoteData {
            sapling_nullifiers_and_positions: HashMap::new(),
            orchard_nullifiers_and_positions: HashMap::new(),
        }
    }
}

impl Default for DecryptedNoteData {
    fn default() -> Self {
        Self::new()
    }
}

// scans a given range and returns all data relevent to the specified keys
// `previous_wallet_block` is the block with height [scan_range.start - 1]
pub(crate) async fn scan<P>(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    parameters: &P,
    scan_range: ScanRange,
    scanning_keys: &ScanningKeys,
    previous_wallet_block: Option<WalletBlock>,
) -> Result<ScanData, ()>
where
    P: Parameters + Sync + Send + 'static,
{
    let compact_blocks = get_compact_block_range(
        fetch_request_sender.clone(),
        scan_range.block_range().clone(),
    )
    .await
    .unwrap();

    let initial_scan_data = InitialScanData::new(
        fetch_request_sender,
        parameters,
        compact_blocks
            .first()
            .expect("compacts blocks should not be empty"),
        previous_wallet_block,
    )
    .await
    .unwrap();

    let (nullifiers, wallet_blocks, _relevent_txids, _note_data, shard_tree_data) =
        scan_compact_blocks(compact_blocks, parameters, scanning_keys, initial_scan_data).unwrap();

    // TODO: scan transactions

    Ok(ScanData {
        nullifiers,
        wallet_blocks,
        shard_tree_data,
    })
}

#[allow(clippy::type_complexity)]
fn scan_compact_blocks<P>(
    compact_blocks: Vec<CompactBlock>,
    parameters: &P,
    scanning_keys: &ScanningKeys,
    initial_scan_data: InitialScanData,
) -> Result<
    (
        NullifierMap,
        BTreeMap<BlockHeight, WalletBlock>,
        HashSet<TxId>,
        DecryptedNoteData,
        ShardTreeData,
    ),
    (),
>
where
    P: Parameters + Sync + Send + 'static,
{
    check_continuity(&compact_blocks, initial_scan_data.previous_block.as_ref()).unwrap();

    let mut runners = trial_decrypt(parameters, scanning_keys, &compact_blocks).unwrap();

    let mut wallet_blocks: BTreeMap<BlockHeight, WalletBlock> = BTreeMap::new();
    let mut nullifiers = NullifierMap::new();
    let mut relevent_txids: HashSet<TxId> = HashSet::new();
    let mut note_data = DecryptedNoteData::new();
    let mut shard_tree_data = ShardTreeData::new(
        Position::from(u64::from(initial_scan_data.sapling_initial_tree_size)),
        Position::from(u64::from(initial_scan_data.orchard_initial_tree_size)),
    );
    let mut sapling_tree_size = initial_scan_data.sapling_initial_tree_size;
    let mut orchard_tree_size = initial_scan_data.orchard_initial_tree_size;
    for block in &compact_blocks {
        let mut transactions = block.vtx.iter().peekable();
        while let Some(transaction) = transactions.next() {
            // collect trial decryption results by transaction
            let incoming_sapling_outputs = runners
                .sapling
                .collect_results(block.hash(), transaction.txid());
            let incoming_orchard_outputs = runners
                .orchard
                .collect_results(block.hash(), transaction.txid());

            // gather the txids of all transactions relevent to the wallet
            // the edge case of transactions that this capability created but did not receive change
            // or create outgoing data is handled when the nullifiers are added and linked
            incoming_sapling_outputs.iter().for_each(|(output_id, _)| {
                relevent_txids.insert(output_id.txid());
            });
            incoming_orchard_outputs.iter().for_each(|(output_id, _)| {
                relevent_txids.insert(output_id.txid());
            });
            // TODO: add outgoing outputs to relevent txids

            collect_nullifiers(&mut nullifiers, block.height(), transaction).unwrap();

            shard_tree_data.sapling_leaves_and_retentions.extend(
                calculate_sapling_leaves_and_retentions(
                    &transaction.outputs,
                    block.height(),
                    transactions.peek().is_none(),
                    &incoming_sapling_outputs,
                )
                .unwrap(),
            );
            shard_tree_data.orchard_leaves_and_retentions.extend(
                calculate_orchard_leaves_and_retentions(
                    &transaction.actions,
                    block.height(),
                    transactions.peek().is_none(),
                    &incoming_orchard_outputs,
                )
                .unwrap(),
            );

            calculate_nullifiers_and_positions(
                sapling_tree_size,
                scanning_keys.sapling(),
                &incoming_sapling_outputs,
                &mut note_data.sapling_nullifiers_and_positions,
            );
            calculate_nullifiers_and_positions(
                orchard_tree_size,
                scanning_keys.orchard(),
                &incoming_orchard_outputs,
                &mut note_data.orchard_nullifiers_and_positions,
            );

            sapling_tree_size += u32::try_from(transaction.outputs.len())
                .expect("should not be more than 2^32 outputs in a transaction");
            orchard_tree_size += u32::try_from(transaction.actions.len())
                .expect("should not be more than 2^32 outputs in a transaction");
        }

        let wallet_block = WalletBlock::from_parts(
            block.height(),
            block.hash(),
            block.prev_hash(),
            block.time,
            block.vtx.iter().map(|tx| tx.txid()).collect(),
            sapling_tree_size,
            orchard_tree_size,
        );

        check_tree_size(block, &wallet_block).unwrap();

        wallet_blocks.insert(wallet_block.block_height(), wallet_block);
    }
    // TODO: map nullifiers

    Ok((
        nullifiers,
        wallet_blocks,
        relevent_txids,
        note_data,
        shard_tree_data,
    ))
}

fn trial_decrypt<P>(
    parameters: &P,
    scanning_keys: &ScanningKeys,
    compact_blocks: &[CompactBlock],
) -> Result<BatchRunners<(), ()>, ()>
where
    P: Parameters + Send + 'static,
{
    // TODO: add outgoing decryption

    let mut runners = BatchRunners::<(), ()>::for_keys(100, scanning_keys);
    for block in compact_blocks {
        runners.add_block(parameters, block.clone()).unwrap();
    }
    runners.flush();

    Ok(runners)
}

// checks height and hash continuity of a batch of compact blocks.
// takes the last wallet compact block of the adjacent lower scan range, if available.
fn check_continuity(
    compact_blocks: &[CompactBlock],
    previous_compact_block: Option<&WalletBlock>,
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

fn check_tree_size(compact_block: &CompactBlock, wallet_block: &WalletBlock) -> Result<(), ()> {
    if let Some(chain_metadata) = &compact_block.chain_metadata {
        if chain_metadata.sapling_commitment_tree_size
            != wallet_block.sapling_commitment_tree_size()
        {
            panic!("sapling tree size is incorrect!")
        }
        if chain_metadata.orchard_commitment_tree_size
            != wallet_block.orchard_commitment_tree_size()
        {
            panic!("orchard tree size is incorrect!")
        }
    }

    Ok(())
}

// calculates nullifiers and positions of incoming decrypted outputs for a given compact transaction and insert into hash map
// `tree_size` is the tree size of the corresponding shielded pool up to - and not including - the compact transaction
// being processed
fn calculate_nullifiers_and_positions<D, K, Nf>(
    tree_size: u32,
    keys: &HashMap<KeyId, K>,
    incoming_decrypted_outputs: &HashMap<OutputId, DecryptedOutput<D, ()>>,
    nullifiers_and_positions: &mut HashMap<OutputId, (Nf, Position)>,
) where
    D: Domain,
    K: ScanningKeyOps<D, Nf>,
{
    incoming_decrypted_outputs
        .iter()
        .for_each(|(output_id, incoming_output)| {
            let position = Position::from(u64::from(
                tree_size + u32::try_from(output_id.output_index()).unwrap(),
            ));
            let key = keys
                .get(&incoming_output.ivk_tag)
                .expect("key should be available as it was used to decrypt output");
            let nullifier = key
                .nf(&incoming_output.note, position)
                .expect("only fvks currently supported");
            nullifiers_and_positions.insert(*output_id, (nullifier, position));
        });
}

// TODO: unify sapling and orchard leaf and retention fns
// calculates the sapling note commitment tree leaves and shardtree retentions for a given compact transaction
fn calculate_sapling_leaves_and_retentions<D: Domain>(
    outputs: &[CompactSaplingOutput],
    block_height: BlockHeight,
    last_outputs_in_block: bool,
    incoming_decrypted_outputs: &HashMap<OutputId, DecryptedOutput<D, ()>>,
) -> Result<Vec<(Node, Retention<BlockHeight>)>, ()> {
    let incoming_output_indices: Vec<usize> = incoming_decrypted_outputs
        .keys()
        .copied()
        .map(|output_id| output_id.output_index())
        .collect();

    if outputs.is_empty() {
        Ok(Vec::new())
    } else {
        let last_output_index = outputs.len() - 1;

        let leaves_and_retentions = outputs
            .iter()
            .enumerate()
            .map(|(output_index, output)| {
                let note_commitment = CompactOutputDescription::try_from(output).unwrap().cmu;
                let leaf = sapling_crypto::Node::from_cmu(&note_commitment);

                let last_output_in_block: bool =
                    last_outputs_in_block && output_index == last_output_index;
                let decrypted: bool = incoming_output_indices.contains(&output_index);
                let retention = match (decrypted, last_output_in_block) {
                    (is_marked, true) => Retention::Checkpoint {
                        id: block_height,
                        is_marked,
                    },
                    (true, false) => Retention::Marked,
                    (false, false) => Retention::Ephemeral,
                };

                (leaf, retention)
            })
            .collect();

        Ok(leaves_and_retentions)
    }
}
// calculates the orchard note commitment tree leaves and shardtree retentions for a given compact transaction
fn calculate_orchard_leaves_and_retentions<D: Domain>(
    actions: &[CompactOrchardAction],
    block_height: BlockHeight,
    last_outputs_in_block: bool,
    incoming_decrypted_outputs: &HashMap<OutputId, DecryptedOutput<D, ()>>,
) -> Result<Vec<(MerkleHashOrchard, Retention<BlockHeight>)>, ()> {
    let incoming_output_indices: Vec<usize> = incoming_decrypted_outputs
        .keys()
        .copied()
        .map(|output_id| output_id.output_index())
        .collect();

    if actions.is_empty() {
        Ok(Vec::new())
    } else {
        let last_output_index = actions.len() - 1;

        let leaves_and_retentions = actions
            .iter()
            .enumerate()
            .map(|(output_index, output)| {
                let note_commitment = CompactAction::try_from(output).unwrap().cmx();
                let leaf = MerkleHashOrchard::from_cmx(&note_commitment);

                let last_output_in_block: bool =
                    last_outputs_in_block && output_index == last_output_index;
                let decrypted: bool = incoming_output_indices.contains(&output_index);
                let retention = match (decrypted, last_output_in_block) {
                    (is_marked, true) => Retention::Checkpoint {
                        id: block_height,
                        is_marked,
                    },
                    (true, false) => Retention::Marked,
                    (false, false) => Retention::Ephemeral,
                };

                (leaf, retention)
            })
            .collect();

        Ok(leaves_and_retentions)
    }
}

// converts and adds the nullifiers from a compact transaction to the nullifier map
fn collect_nullifiers(
    nullifier_map: &mut NullifierMap,
    block_height: BlockHeight,
    transaction: &CompactTx,
) -> Result<(), ()> {
    transaction
        .spends
        .iter()
        .map(|spend| sapling_crypto::Nullifier::from_slice(spend.nf.as_slice()).unwrap())
        .for_each(|nullifier| {
            nullifier_map
                .sapling_mut()
                .insert(nullifier, (block_height, transaction.txid()));
        });
    transaction
        .actions
        .iter()
        .map(|action| {
            orchard::note::Nullifier::from_bytes(action.nullifier.as_slice().try_into().unwrap())
                .unwrap()
        })
        .for_each(|nullifier| {
            nullifier_map
                .orchard_mut()
                .insert(nullifier, (block_height, transaction.txid()));
        });
    Ok(())
}

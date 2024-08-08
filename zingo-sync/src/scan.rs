use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
};

use incrementalmerkletree::{Position, Retention};
use orchard::{keys::Scope, note_encryption::CompactAction, tree::MerkleHashOrchard};
use sapling_crypto::{note_encryption::CompactOutputDescription, Node};
use tokio::sync::mpsc::UnboundedSender;
use zcash_client_backend::{
    data_api::scanning::ScanRange,
    proto::compact_formats::{CompactBlock, CompactOrchardAction, CompactSaplingOutput},
    scanning::ScanningKeys,
};
use zcash_note_encryption::Domain;
use zcash_primitives::{
    block::BlockHash,
    consensus::{BlockHeight, NetworkUpgrade, Parameters},
    transaction::TxId,
    zip32::AccountId,
};

use crate::{
    client::{self, get_compact_block_range, FetchRequest},
    primitives::{OutputId, WalletCompactBlock},
};

use self::runners::{BatchRunners, DecryptedOutput};

pub(crate) mod runners;

type KeyId = (AccountId, Scope);

struct InitialScanData<'a> {
    previous_block: Option<&'a WalletCompactBlock>,
    sapling_initial_tree_size: u32,
    orchard_initial_tree_size: u32,
}

impl InitialScanData<'_> {
    async fn new<P>(
        fetch_request_sender: UnboundedSender<FetchRequest>,
        parameters: &P,
        first_block: &CompactBlock,
    ) -> Result<Self, ()>
    where
        P: Parameters + Send + 'static,
    {
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
            previous_block,
            sapling_initial_tree_size,
            orchard_initial_tree_size,
        })
    }
}

pub(crate) struct ScanResults {
    relevent_txids: HashSet<TxId>,
    sapling_initial_position: Position,
    sapling_leaves_and_retentions: Vec<(Node, Retention<BlockHeight>)>,
    sapling_nullifiers_and_positions: HashMap<OutputId, (sapling_crypto::Nullifier, Position)>,
    orchard_initial_position: Position,
    orchard_leaves_and_retentions: Vec<(MerkleHashOrchard, Retention<BlockHeight>)>,
    orchard_nullifiers_and_positions: HashMap<OutputId, (orchard::note::Nullifier, Position)>,
}
impl ScanResults {
    fn new(sapling_initial_position: Position, orchard_initial_position: Position) -> Self {
        ScanResults {
            relevent_txids: HashSet::new(),
            sapling_initial_position,
            sapling_leaves_and_retentions: Vec::new(),
            sapling_nullifiers_and_positions: HashMap::new(),
            orchard_initial_position,
            orchard_leaves_and_retentions: Vec::new(),
            orchard_nullifiers_and_positions: HashMap::new(),
        }
    }
}

pub(crate) async fn scan<P>(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    parameters: &P,
    scanning_keys: &ScanningKeys<AccountId, KeyId>,
    scan_range: ScanRange,
) -> Result<ScanResults, ()>
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
        parameters,
        compact_blocks
            .first()
            .expect("compacts blocks should not be empty"),
    )
    .await
    .unwrap();

    check_continuity(&compact_blocks, initial_scan_data.previous_block).unwrap();

    let mut runners = trial_decrypt(parameters, scanning_keys, &compact_blocks).unwrap();

    let mut scan_results = ScanResults::new(
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
                scan_results.relevent_txids.insert(output_id.txid());
            });
            incoming_orchard_outputs.iter().for_each(|(output_id, _)| {
                scan_results.relevent_txids.insert(output_id.txid());
            });
            // TODO: add outgoing outputs to relevent txids

            scan_results.sapling_leaves_and_retentions.extend(
                calculate_sapling_leaves_and_retentions(
                    &transaction.outputs,
                    block.height(),
                    transactions.peek().is_none(),
                    &incoming_sapling_outputs,
                )
                .unwrap(),
            );
            scan_results.orchard_leaves_and_retentions.extend(
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
                &mut scan_results.sapling_nullifiers_and_positions,
            );
            calculate_nullifiers_and_positions(
                orchard_tree_size,
                scanning_keys.orchard(),
                &incoming_orchard_outputs,
                &mut scan_results.orchard_nullifiers_and_positions,
            );

            sapling_tree_size += u32::try_from(transaction.outputs.len())
                .expect("should not be more than 2^32 outputs in a transaction");
            orchard_tree_size += u32::try_from(transaction.actions.len())
                .expect("should not be more than 2^32 outputs in a transaction");
        }
        // TODO: check tree size matches chain_metadata if available
    }
    // TODO: map nullifiers and write compact blocks

    Ok(scan_results)
}

fn trial_decrypt<P>(
    parameters: &P,
    scanning_keys: &ScanningKeys<AccountId, KeyId>,
    compact_blocks: &[CompactBlock],
) -> Result<BatchRunners<KeyId, (), ()>, ()>
where
    P: Parameters + Send + 'static,
{
    // TODO: add outgoing decryption

    let mut runners = BatchRunners::<KeyId, (), ()>::for_keys(100, scanning_keys);
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

// calculates nullifiers and positions for a given compact transaction and insert into hash map
// `tree_size` is the tree size of the corresponding shielded pool up to - and not including - the compact transaction
// being processed
fn calculate_nullifiers_and_positions<D, K, Nf>(
    tree_size: u32,
    keys: &HashMap<KeyId, K>,
    incoming_outputs: &HashMap<OutputId, DecryptedOutput<KeyId, D, ()>>,
    nullifiers_and_positions: &mut HashMap<OutputId, (Nf, Position)>,
) where
    D: Domain,
    K: zcash_client_backend::scanning::ScanningKeyOps<D, AccountId, Nf>,
{
    incoming_outputs
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
    decrypted_incoming_outputs: &HashMap<OutputId, DecryptedOutput<KeyId, D, ()>>,
) -> Result<Vec<(Node, Retention<BlockHeight>)>, ()> {
    let incoming_output_indices: Vec<usize> = decrypted_incoming_outputs
        .keys()
        .copied()
        .map(|output_id| output_id.output_index())
        .collect();
    let last_output_index = outputs.len() - 1;

    Ok(outputs
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
        .collect())
}
// calculates the orchard note commitment tree leaves and shardtree retentions for a given compact transaction
fn calculate_orchard_leaves_and_retentions<D: Domain>(
    actions: &[CompactOrchardAction],
    block_height: BlockHeight,
    last_outputs_in_block: bool,
    decrypted_incoming_outputs: &HashMap<OutputId, DecryptedOutput<KeyId, D, ()>>,
) -> Result<Vec<(MerkleHashOrchard, Retention<BlockHeight>)>, ()> {
    let incoming_output_indices: Vec<usize> = decrypted_incoming_outputs
        .keys()
        .copied()
        .map(|output_id| output_id.output_index())
        .collect();
    let last_output_index = actions.len() - 1;

    Ok(actions
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
        .collect())
}

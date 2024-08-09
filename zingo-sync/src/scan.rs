use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
};

use incrementalmerkletree::Position;
use orchard::keys::Scope;
use tokio::sync::mpsc::UnboundedSender;
use zcash_client_backend::{
    data_api::scanning::ScanRange,
    proto::compact_formats::{CompactBlock, CompactTx},
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

use self::runners::BatchRunners;

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

pub(crate) async fn scan<P>(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    parameters: &P,
    scanning_keys: &ScanningKeys<AccountId, KeyId>,
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
        parameters,
        compact_blocks
            .first()
            .expect("compacts blocks should not be empty"),
    )
    .await
    .unwrap();

    check_continuity(&compact_blocks, initial_scan_data.previous_block).unwrap();

    let (incoming_sapling_outputs, incoming_orchard_outputs) =
        trial_decrypt(parameters, scanning_keys, &compact_blocks).unwrap();

    let mut sapling_nullifiers_and_positions: HashMap<
        OutputId,
        (sapling_crypto::Nullifier, Position),
    > = HashMap::new();
    let mut orchard_nullifiers_and_positions: HashMap<
        OutputId,
        (orchard::note::Nullifier, Position),
    > = HashMap::new();
    let mut sapling_tree_size = initial_scan_data.sapling_initial_tree_size;
    let mut orchard_tree_size = initial_scan_data.orchard_initial_tree_size;
    for block in compact_blocks {
        // let zip212_enforcement = zip212_enforcement(parameters, block.height());
        for transaction in block.vtx {
            // TODO: add note commitment and retentions

            calculate_nullifiers_and_positions(
                &transaction,
                sapling_tree_size,
                scanning_keys.sapling(),
                &incoming_sapling_outputs,
                &mut sapling_nullifiers_and_positions,
            );
            calculate_nullifiers_and_positions(
                &transaction,
                orchard_tree_size,
                scanning_keys.orchard(),
                &incoming_orchard_outputs,
                &mut orchard_nullifiers_and_positions,
            );

            sapling_tree_size += u32::try_from(transaction.outputs.len())
                .expect("should not be more than 2^32 outputs in a transaction");
            orchard_tree_size += u32::try_from(transaction.actions.len())
                .expect("should not be more than 2^32 outputs in a transaction");
        }
    }
    // TODO: map nullifiers and write compact blocks

    // gather the IDs of all transactions relevent to the wallet
    // the edge case of transactions that this capability created but did not receive change
    // or create outgoing data is handled when the nullifiers are added and linked
    let mut relevent_txids: HashSet<TxId> = HashSet::new();
    incoming_sapling_outputs
        .iter()
        .for_each(|(output_id, _, _)| {
            relevent_txids.insert(output_id.txid());
        });
    incoming_orchard_outputs
        .iter()
        .for_each(|(output_id, _, _)| {
            relevent_txids.insert(output_id.txid());
        });

    Ok(())
}

// TODO: reduce type complexity once trial decryption is finalised
#[allow(clippy::type_complexity)]
fn trial_decrypt<P>(
    parameters: &P,
    scanning_keys: &ScanningKeys<AccountId, KeyId>,
    compact_blocks: &[CompactBlock],
) -> Result<
    (
        Vec<(OutputId, sapling_crypto::Note, KeyId)>,
        Vec<(OutputId, orchard::Note, KeyId)>,
    ),
    (),
>
where
    P: Parameters + Send + 'static,
{
    let mut runners = BatchRunners::<_, (), ()>::for_keys(100, scanning_keys);
    for block in compact_blocks {
        runners.add_block(parameters, block.clone()).unwrap();
    }
    runners.flush();

    let mut incoming_sapling_outputs: Vec<(OutputId, sapling_crypto::Note, KeyId)> = Vec::new();
    let mut incoming_orchard_outputs: Vec<(OutputId, orchard::Note, KeyId)> = Vec::new();

    // TODO: add outgoing decryption
    for block in compact_blocks {
        for transaction in block.vtx.iter() {
            let decrypted_sapling_outputs = runners
                .sapling
                .collect_results(block.hash(), transaction.txid());
            decrypted_sapling_outputs
                .iter()
                .for_each(|((txid, output_index), output)| {
                    incoming_sapling_outputs.push((
                        OutputId::from_parts(*txid, *output_index),
                        output.note.clone(),
                        output.ivk_tag,
                    ));
                });

            let decrypted_orchard_outputs = runners
                .orchard
                .collect_results(block.hash(), transaction.txid());
            decrypted_orchard_outputs
                .iter()
                .for_each(|((txid, output_index), output)| {
                    incoming_orchard_outputs.push((
                        OutputId::from_parts(*txid, *output_index),
                        output.note,
                        output.ivk_tag,
                    ));
                });
        }
    }

    Ok((incoming_sapling_outputs, incoming_orchard_outputs))
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

// calculates nullifiers and positions for a given compact transaction
fn calculate_nullifiers_and_positions<D, K, Nf>(
    transaction: &CompactTx,
    tx_initial_tree_size: u32,
    keys: &HashMap<KeyId, K>,
    incoming_outputs: &[(OutputId, D::Note, KeyId)],
    nullifiers_and_positions: &mut HashMap<OutputId, (Nf, Position)>,
) where
    D: Domain,
    K: zcash_client_backend::scanning::ScanningKeyOps<D, AccountId, Nf>,
{
    incoming_outputs
        .iter()
        .filter(|(output_id, _, _)| output_id.txid() == transaction.txid())
        .for_each(|(output_id, note, key_id)| {
            let position = Position::from(u64::from(
                tx_initial_tree_size + u32::try_from(output_id.output_index()).unwrap(),
            ));
            let key = keys
                .get(key_id)
                .expect("key should be available as it was used to decrypt output");
            let nullifier = key
                .nf(note, position)
                .expect("only fvks currently supported");
            nullifiers_and_positions.insert(*output_id, (nullifier, position));
        });
}

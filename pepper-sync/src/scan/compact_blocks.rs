use std::{
    cmp,
    collections::{BTreeMap, BTreeSet, HashMap},
};

use incrementalmerkletree::{Marking, Position, Retention};
use orchard::{note_encryption::CompactAction, tree::MerkleHashOrchard};
use sapling_crypto::{note_encryption::CompactOutputDescription, Node};
use tokio::sync::mpsc;
use zcash_client_backend::proto::compact_formats::{
    CompactBlock, CompactOrchardAction, CompactSaplingOutput, CompactTx,
};
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_note_encryption::Domain;
use zcash_primitives::{
    block::BlockHash,
    consensus::{self, BlockHeight, Parameters},
    zip32::AccountId,
};

use crate::{
    client::{self, FetchRequest},
    error::{ClientError, ContinuityError, ScanError},
    keys::{KeyId, ScanningKeyOps, ScanningKeys},
    wallet::{NullifierMap, OutputId, TreeBounds, WalletBlock},
    witness::WitnessData,
    MAX_BATCH_OUTPUTS,
};

use self::runners::{BatchRunners, DecryptedOutput};

use super::{DecryptedNoteData, InitialScanData, ScanData};

mod runners;

const TRIAL_DECRYPT_TASK_SIZE: usize = MAX_BATCH_OUTPUTS / 16;

pub(super) fn scan_compact_blocks<P>(
    compact_blocks: Vec<CompactBlock>,
    parameters: &P,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    initial_scan_data: InitialScanData,
) -> Result<ScanData, ScanError>
where
    P: Parameters + Sync + Send + 'static,
{
    check_continuity(
        &compact_blocks,
        initial_scan_data.start_seam_block.as_ref(),
        initial_scan_data.end_seam_block.as_ref(),
    )?;

    let scanning_keys = ScanningKeys::from_account_ufvks(ufvks.clone());
    let mut runners = trial_decrypt(parameters, &scanning_keys, &compact_blocks).unwrap();

    let mut wallet_blocks: BTreeMap<BlockHeight, WalletBlock> = BTreeMap::new();
    let mut nullifiers = NullifierMap::new();
    let mut decrypted_locators = BTreeSet::new();
    let mut decrypted_note_data = DecryptedNoteData::new();
    let mut witness_data = WitnessData::new(
        Position::from(u64::from(initial_scan_data.sapling_initial_tree_size)),
        Position::from(u64::from(initial_scan_data.orchard_initial_tree_size)),
    );
    let mut sapling_initial_tree_size;
    let mut orchard_initial_tree_size;
    let mut sapling_final_tree_size = initial_scan_data.sapling_initial_tree_size;
    let mut orchard_final_tree_size = initial_scan_data.orchard_initial_tree_size;
    for block in &compact_blocks {
        sapling_initial_tree_size = sapling_final_tree_size;
        orchard_initial_tree_size = orchard_final_tree_size;

        let block_height = block.height();

        let mut transactions = block.vtx.iter().peekable();
        while let Some(transaction) = transactions.next() {
            // collect trial decryption results by transaction
            let incoming_sapling_outputs = runners
                .sapling
                .collect_results(block.hash(), transaction.txid());
            let incoming_orchard_outputs = runners
                .orchard
                .collect_results(block.hash(), transaction.txid());

            // gather the txids of all transactions relevant to the wallet
            // the edge case of transactions that this capability created but did not receive change
            // or create outgoing data is handled when the nullifiers are added and linked
            incoming_sapling_outputs.iter().for_each(|(output_id, _)| {
                decrypted_locators.insert((block_height, output_id.txid()));
            });
            incoming_orchard_outputs.iter().for_each(|(output_id, _)| {
                decrypted_locators.insert((block_height, output_id.txid()));
            });

            collect_nullifiers(&mut nullifiers, block.height(), transaction).unwrap();

            witness_data.sapling_leaves_and_retentions.extend(
                calculate_sapling_leaves_and_retentions(
                    &transaction.outputs,
                    block.height(),
                    transactions.peek().is_none(),
                    &incoming_sapling_outputs,
                )
                .unwrap(),
            );
            witness_data.orchard_leaves_and_retentions.extend(
                calculate_orchard_leaves_and_retentions(
                    &transaction.actions,
                    block.height(),
                    transactions.peek().is_none(),
                    &incoming_orchard_outputs,
                )
                .unwrap(),
            );

            calculate_nullifiers_and_positions(
                sapling_final_tree_size,
                &scanning_keys.sapling,
                &incoming_sapling_outputs,
                &mut decrypted_note_data.sapling_nullifiers_and_positions,
            );
            calculate_nullifiers_and_positions(
                orchard_final_tree_size,
                &scanning_keys.orchard,
                &incoming_orchard_outputs,
                &mut decrypted_note_data.orchard_nullifiers_and_positions,
            );

            sapling_final_tree_size += u32::try_from(transaction.outputs.len())
                .expect("should not be more than 2^32 outputs in a transaction");
            orchard_final_tree_size += u32::try_from(transaction.actions.len())
                .expect("should not be more than 2^32 outputs in a transaction");
        }

        let wallet_block = WalletBlock {
            block_height: block.height(),
            block_hash: block.hash(),
            prev_hash: block.prev_hash(),
            time: block.time,
            txids: block
                .vtx
                .iter()
                .map(|transaction| transaction.txid())
                .collect(),
            tree_bounds: TreeBounds {
                sapling_initial_tree_size,
                sapling_final_tree_size,
                orchard_initial_tree_size,
                orchard_final_tree_size,
            },
        };

        check_tree_size(block, &wallet_block).unwrap();

        wallet_blocks.insert(wallet_block.block_height(), wallet_block);
    }

    Ok(ScanData {
        nullifiers,
        wallet_blocks,
        decrypted_locators,
        decrypted_note_data,
        witness_data,
    })
}

fn trial_decrypt<P>(
    parameters: &P,
    scanning_keys: &ScanningKeys,
    compact_blocks: &[CompactBlock],
) -> Result<BatchRunners<(), ()>, ()>
where
    P: Parameters + Send + 'static,
{
    let mut runners = BatchRunners::<(), ()>::for_keys(TRIAL_DECRYPT_TASK_SIZE, scanning_keys);
    for block in compact_blocks {
        runners.add_block(parameters, block.clone()).unwrap();
    }
    runners.flush();

    Ok(runners)
}

/// Checks height and hash continuity of a batch of compact blocks.
///
/// If available, also checks continuity with the blocks adjacent to the `compact_blocks` forming the start and end
/// seams of the scan ranges.
fn check_continuity(
    compact_blocks: &[CompactBlock],
    start_seam_block: Option<&WalletBlock>,
    end_seam_block: Option<&WalletBlock>,
) -> Result<(), ContinuityError> {
    let mut prev_height: Option<BlockHeight> = None;
    let mut prev_hash: Option<BlockHash> = None;

    if let Some(start_seam_block) = start_seam_block {
        prev_height = Some(start_seam_block.block_height());
        prev_hash = Some(start_seam_block.block_hash());
    }

    for block in compact_blocks {
        if let Some(prev_height) = prev_height {
            if block.height() != prev_height + 1 {
                return Err(ContinuityError::HeightDiscontinuity {
                    height: block.height(),
                    previous_block_height: prev_height,
                });
            }
        }

        if let Some(prev_hash) = prev_hash {
            if block.prev_hash() != prev_hash {
                return Err(ContinuityError::HashDiscontinuity {
                    height: block.height(),
                    prev_hash: block.prev_hash(),
                    previous_block_hash: prev_hash,
                });
            }
        }

        prev_height = Some(block.height());
        prev_hash = Some(block.hash());
    }

    if let Some(end_seam_block) = end_seam_block {
        let prev_height = prev_height.expect("compact blocks should not be empty");
        if end_seam_block.block_height() != prev_height + 1 {
            return Err(ContinuityError::HeightDiscontinuity {
                height: end_seam_block.block_height(),
                previous_block_height: prev_height,
            });
        }

        let prev_hash = prev_hash.expect("compact blocks should not be empty");
        if end_seam_block.prev_hash() != prev_hash {
            return Err(ContinuityError::HashDiscontinuity {
                height: end_seam_block.block_height(),
                prev_hash: end_seam_block.prev_hash(),
                previous_block_hash: prev_hash,
            });
        }
    }

    Ok(())
}

fn check_tree_size(compact_block: &CompactBlock, wallet_block: &WalletBlock) -> Result<(), ()> {
    if let Some(chain_metadata) = &compact_block.chain_metadata {
        if chain_metadata.sapling_commitment_tree_size
            != wallet_block.tree_bounds().sapling_final_tree_size
        {
            #[cfg(feature = "darkside_test")]
            {
                tracing::error!("darkside compact block sapling tree size incorrect.\nwallet block: {}\ncompact_block: {}", wallet_block.tree_bounds().sapling_final_tree_size, compact_block.chain_metadata.unwrap().sapling_commitment_tree_size);
                return Ok(());
            }

            #[cfg(not(feature = "darkside_test"))]
            panic!("sapling tree size is incorrect!")
        }
        if chain_metadata.orchard_commitment_tree_size
            != wallet_block.tree_bounds().orchard_final_tree_size
        {
            #[cfg(feature = "darkside_test")]
            {
                tracing::error!("darkside compact block orchard tree size incorrect.\nwallet block: {}\ncompact_block: {}", wallet_block.tree_bounds().orchard_final_tree_size, compact_block.chain_metadata.unwrap().orchard_commitment_tree_size);
                return Ok(());
            }

            #[cfg(not(feature = "darkside_test"))]
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
            let position =
                Position::from(u64::from(tree_size + u32::from(output_id.output_index())));
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
    let incoming_output_indexes = incoming_decrypted_outputs
        .keys()
        .copied()
        .map(|output_id| output_id.output_index())
        .collect::<Vec<_>>();

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
                let decrypted: bool = incoming_output_indexes.contains(&(output_index as u16));
                let retention = match (decrypted, last_output_in_block) {
                    (decrypted, true) => Retention::Checkpoint {
                        id: block_height,
                        marking: if decrypted {
                            Marking::Marked
                        } else {
                            Marking::None
                        },
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
    let incoming_output_indexes = incoming_decrypted_outputs
        .keys()
        .copied()
        .map(|output_id| output_id.output_index())
        .collect::<Vec<_>>();

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
                let decrypted: bool = incoming_output_indexes.contains(&(output_index as u16));
                let retention = match (decrypted, last_output_in_block) {
                    (is_marked, true) => Retention::Checkpoint {
                        id: block_height,
                        marking: if is_marked {
                            Marking::Marked
                        } else {
                            Marking::None
                        },
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
                .sapling
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
                .orchard
                .insert(nullifier, (block_height, transaction.txid()));
        });
    Ok(())
}

pub(crate) async fn calculate_block_tree_bounds(
    consensus_parameters: &impl consensus::Parameters,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    compact_block: &CompactBlock,
) -> Result<TreeBounds, ClientError> {
    let (sapling_final_tree_size, orchard_final_tree_size) =
        if let Some(chain_metadata) = compact_block.chain_metadata {
            (
                chain_metadata.sapling_commitment_tree_size,
                chain_metadata.orchard_commitment_tree_size,
            )
        } else {
            let sapling_activation_height = consensus_parameters
                .activation_height(consensus::NetworkUpgrade::Sapling)
                .expect("should have some sapling activation height");

            match compact_block.height().cmp(&sapling_activation_height) {
                cmp::Ordering::Greater => {
                    let frontiers =
                        client::get_frontiers(fetch_request_sender.clone(), compact_block.height())
                            .await?;
                    (
                        frontiers
                            .final_sapling_tree()
                            .tree_size()
                            .try_into()
                            .expect("should not be more than 2^32 note commitments in the tree!"),
                        frontiers
                            .final_orchard_tree()
                            .tree_size()
                            .try_into()
                            .expect("should not be more than 2^32 note commitments in the tree!"),
                    )
                }
                cmp::Ordering::Equal => (0, 0),
                cmp::Ordering::Less => panic!("pre-sapling not supported!"),
            }
        };

    let sapling_output_count: u32 = compact_block
        .vtx
        .iter()
        .map(|tx| tx.outputs.len())
        .sum::<usize>()
        .try_into()
        .expect("Sapling output count cannot exceed a u32");
    let orchard_output_count: u32 = compact_block
        .vtx
        .iter()
        .map(|tx| tx.actions.len())
        .sum::<usize>()
        .try_into()
        .expect("Sapling output count cannot exceed a u32");

    Ok(TreeBounds {
        sapling_initial_tree_size: sapling_final_tree_size.saturating_sub(sapling_output_count),
        sapling_final_tree_size,
        orchard_initial_tree_size: orchard_final_tree_size.saturating_sub(orchard_output_count),
        orchard_final_tree_size,
    })
}

use std::{
    cmp,
    collections::{BTreeMap, BTreeSet, HashMap},
};

use incrementalmerkletree::{Marking, Position, Retention};
use orchard::tree::MerkleHashOrchard;
use sapling_crypto::Node;
use tokio::sync::mpsc;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_note_encryption::Domain;
use zcash_primitives::block::BlockHash;
use zcash_protocol::consensus::{self, BlockHeight};
use zingo_netutils::lightwallet_protocol::{
    CompactBlock, CompactOrchardAction, CompactSaplingOutput,
};
use zip32::AccountId;

use crate::{
    client::{self, FetchRequest},
    error::{ContinuityError, ScanError, ServerError},
    keys::{KeyId, ScanningKeyOps, ScanningKeys},
    utils::{
        get_compact_action, get_compact_block_hash, get_compact_block_height,
        get_compact_block_prev_hash, get_compact_output_description, get_compact_tx_txid,
    },
    wallet::{
        NullifierMap, OutputId, RevokedTestimony, ScanTarget, TreeBounds, TreeBoundsProvenance,
        WalletBlock,
    },
    witness::WitnessData,
};

use zcash_protocol::{PoolType, ShieldedPool};

use self::runners::{BatchRunners, DecryptedOutput};

use super::{DecryptedNoteData, InitialScanData, ScanData, collect_nullifiers};

mod runners;

pub(super) fn scan_compact_blocks<P>(
    compact_blocks: Vec<CompactBlock>,
    consensus_parameters: &P,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    initial_scan_data: InitialScanData,
    trial_decrypt_task_size: usize,
) -> Result<ScanData, ScanError>
where
    P: consensus::Parameters + Sync + Send + 'static,
{
    check_continuity(
        &compact_blocks,
        initial_scan_data.start_seam_block.as_ref(),
        initial_scan_data.end_seam_block.as_ref(),
    )?;

    let scanning_keys = ScanningKeys::from_account_ufvks(ufvks.clone());
    let mut runners = trial_decrypt(
        consensus_parameters,
        &scanning_keys,
        &compact_blocks,
        trial_decrypt_task_size,
    )?;

    let mut wallet_blocks: BTreeMap<BlockHeight, WalletBlock> = BTreeMap::new();
    let mut nullifiers = NullifierMap::new();
    let mut decrypted_scan_targets = BTreeSet::new();
    let mut decrypted_note_data = DecryptedNoteData::new();
    let mut witness_data = WitnessData::new(
        Position::from(u64::from(initial_scan_data.sapling_initial_tree_size)),
        Position::from(u64::from(initial_scan_data.orchard_initial_tree_size)),
        Position::from(u64::from(initial_scan_data.ironwood_initial_tree_size)),
    );
    let mut sapling_initial_tree_size;
    let mut orchard_initial_tree_size;
    let mut ironwood_initial_tree_size;
    let mut sapling_final_tree_size = initial_scan_data.sapling_initial_tree_size;
    let mut orchard_final_tree_size = initial_scan_data.orchard_initial_tree_size;
    let mut ironwood_final_tree_size = initial_scan_data.ironwood_initial_tree_size;
    for block in &compact_blocks {
        sapling_initial_tree_size = sapling_final_tree_size;
        orchard_initial_tree_size = orchard_final_tree_size;
        ironwood_initial_tree_size = ironwood_final_tree_size;

        let block_height = get_compact_block_height(block);

        for transaction in &block.vtx {
            // collect trial decryption results by transaction
            let incoming_sapling_outputs = runners.sapling.collect_results(
                get_compact_block_hash(block),
                get_compact_tx_txid(transaction),
            );
            let incoming_orchard_outputs = runners.orchard.collect_results(
                get_compact_block_hash(block),
                get_compact_tx_txid(transaction),
            );
            let incoming_ironwood_outputs = runners.ironwood.collect_results(
                get_compact_block_hash(block),
                get_compact_tx_txid(transaction),
            );

            // gather the txids of all transactions relevant to the wallet
            // the edge case of transactions that this capability created but did not receive change
            // or create outgoing data is handled when the nullifiers are added and linked
            for output_id in incoming_sapling_outputs.keys() {
                decrypted_scan_targets.insert(ScanTarget {
                    block_height,
                    txid: output_id.txid(),
                    narrow_scan_area: false,
                });
            }
            for output_id in incoming_orchard_outputs.keys() {
                decrypted_scan_targets.insert(ScanTarget {
                    block_height,
                    txid: output_id.txid(),
                    narrow_scan_area: false,
                });
            }
            for output_id in incoming_ironwood_outputs.keys() {
                decrypted_scan_targets.insert(ScanTarget {
                    block_height,
                    txid: output_id.txid(),
                    narrow_scan_area: false,
                });
            }

            collect_nullifiers(
                &mut nullifiers,
                get_compact_block_height(block),
                transaction,
            )?;

            witness_data.sapling_leaves_and_retentions.extend(
                calculate_sapling_leaves_and_retentions(
                    &transaction.outputs,
                    &incoming_sapling_outputs,
                )?,
            );
            witness_data.orchard_leaves_and_retentions.extend(
                calculate_orchard_leaves_and_retentions(
                    &transaction.actions,
                    &incoming_orchard_outputs,
                )?,
            );
            witness_data.ironwood_leaves_and_retentions.extend(
                calculate_orchard_leaves_and_retentions(
                    &transaction.ironwood_actions,
                    &incoming_ironwood_outputs,
                )?,
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
            calculate_nullifiers_and_positions(
                ironwood_final_tree_size,
                &scanning_keys.ironwood,
                &incoming_ironwood_outputs,
                &mut decrypted_note_data.ironwood_nullifiers_and_positions,
            );

            sapling_final_tree_size += u32::try_from(transaction.outputs.len())
                .expect("should not be more than 2^32 outputs in a transaction");
            orchard_final_tree_size += u32::try_from(transaction.actions.len())
                .expect("should not be more than 2^32 outputs in a transaction");
            ironwood_final_tree_size += u32::try_from(transaction.ironwood_actions.len())
                .expect("should not be more than 2^32 outputs in a transaction");
        }

        set_checkpoint_retentions(
            block_height,
            &mut witness_data.sapling_leaves_and_retentions,
        );
        set_checkpoint_retentions(
            block_height,
            &mut witness_data.orchard_leaves_and_retentions,
        );
        set_checkpoint_retentions(
            block_height,
            &mut witness_data.ironwood_leaves_and_retentions,
        );

        let wallet_block = WalletBlock {
            block_height: get_compact_block_height(block),
            block_hash: get_compact_block_hash(block),
            prev_hash: get_compact_block_prev_hash(block),
            time: block.time,
            txids: block.vtx.iter().map(get_compact_tx_txid).collect(),
            tree_bounds: TreeBounds {
                sapling_initial_tree_size,
                sapling_final_tree_size,
                orchard_initial_tree_size,
                orchard_final_tree_size,
                ironwood_initial_tree_size,
                ironwood_final_tree_size,
                provenance: TreeBoundsProvenance::Ironwood,
                revoked_testimony: RevokedTestimony::NONE,
            },
        };

        check_tree_size(block, &wallet_block)?;

        wallet_blocks.insert(wallet_block.block_height(), wallet_block);
    }

    Ok(ScanData {
        nullifiers,
        wallet_blocks,
        decrypted_scan_targets,
        decrypted_note_data,
        witness_data,
    })
}

fn trial_decrypt<P>(
    consensus_parameters: &P,
    scanning_keys: &ScanningKeys,
    compact_blocks: &[CompactBlock],
    trial_decrypt_task_size: usize,
) -> Result<BatchRunners<(), (), ()>, ScanError>
where
    P: consensus::Parameters + Send + 'static,
{
    let mut runners = BatchRunners::<(), (), ()>::for_keys(trial_decrypt_task_size, scanning_keys);
    for block in compact_blocks {
        runners.add_block(consensus_parameters, block.clone())?;
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
        if let Some(prev_height) = prev_height
            && get_compact_block_height(block) != prev_height + 1
        {
            return Err(ContinuityError::HeightDiscontinuity {
                height: get_compact_block_height(block),
                previous_block_height: prev_height,
            });
        }

        if let Some(prev_hash) = prev_hash
            && get_compact_block_prev_hash(block) != prev_hash
        {
            return Err(ContinuityError::HashDiscontinuity {
                height: get_compact_block_height(block),
                prev_hash: get_compact_block_prev_hash(block),
                previous_block_hash: prev_hash,
            });
        }

        prev_height = Some(get_compact_block_height(block));
        prev_hash = Some(get_compact_block_hash(block));
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

/// Checks every pool's commitment tree size against the chain.
///
/// A block's metadata size is the chain's own count of that pool's note
/// commitments up to and including the block, so the wallet's count of the
/// outputs it was served must equal it. Two ways to disagree, told apart by
/// what the block actually served:
///
/// Metadata reporting nothing while the block serves outputs for that pool
/// means the server does not serve that pool. Failing sync against such a
/// server would strand every wallet using it, so this warns and carries on.
/// Any other disagreement means the wallet's own history for that pool
/// cannot account for the chain, which the caller heals by rescanning that
/// pool. This holds for every pool, whether or not it activated within the
/// wallet's lifetime, and on every network.
fn check_tree_size(
    compact_block: &CompactBlock,
    wallet_block: &WalletBlock,
) -> Result<(), ScanError> {
    let Some(chain_metadata) = &compact_block.chain_metadata else {
        return Ok(());
    };
    let tree_bounds = wallet_block.tree_bounds();

    for (pool, metadata_size, calculated_size, served_outputs) in [
        (
            ShieldedPool::Sapling,
            chain_metadata.sapling_commitment_tree_size,
            tree_bounds.sapling_final_tree_size,
            served_outputs(compact_block, ShieldedPool::Sapling),
        ),
        (
            ShieldedPool::Orchard,
            chain_metadata.orchard_commitment_tree_size,
            tree_bounds.orchard_final_tree_size,
            served_outputs(compact_block, ShieldedPool::Orchard),
        ),
        (
            ShieldedPool::Ironwood,
            chain_metadata.ironwood_commitment_tree_size,
            tree_bounds.ironwood_final_tree_size,
            served_outputs(compact_block, ShieldedPool::Ironwood),
        ),
    ] {
        if metadata_size == calculated_size {
            continue;
        }

        if metadata_size == 0 && served_outputs > 0 {
            tracing::warn!(
                "{pool:?} outputs served at block {} against a chain metadata size of zero: \
                 this server does not serve {pool:?}",
                wallet_block.block_height(),
            );
            continue;
        }

        return Err(ScanError::IncorrectTreeSize {
            shielded_protocol: PoolType::Shielded(pool),
            block_metadata_size: metadata_size,
            calculated_size,
        });
    }

    Ok(())
}

/// The number of `pool` outputs the block serves.
fn served_outputs(compact_block: &CompactBlock, pool: ShieldedPool) -> usize {
    compact_block
        .vtx
        .iter()
        .map(|transaction| match pool {
            ShieldedPool::Sapling => transaction.outputs.len(),
            ShieldedPool::Orchard => transaction.actions.len(),
            ShieldedPool::Ironwood => transaction.ironwood_actions.len(),
        })
        .sum()
}

/// Calculates nullifiers and positions of incoming decrypted outputs for a given compact transaction and insert into hash map
/// `tree_size` is the tree size of the corresponding shielded pool up to - and not including - the compact transaction
/// being processed
fn calculate_nullifiers_and_positions<D, K, Nf>(
    tree_size: u32,
    keys: &HashMap<KeyId, K>,
    incoming_decrypted_outputs: &HashMap<OutputId, DecryptedOutput<D, ()>>,
    nullifiers_and_positions: &mut HashMap<OutputId, (Nf, Position)>,
) where
    D: Domain,
    K: ScanningKeyOps<D, Nf>,
{
    for (output_id, incoming_output) in incoming_decrypted_outputs {
        let position = Position::from(u64::from(tree_size + output_id.output_index()));
        let key = keys
            .get(&incoming_output.ivk_tag)
            .expect("key should be available as it was used to decrypt output");
        let nullifier = key
            .nf(&incoming_output.note, position)
            .expect("only fvks currently supported");
        nullifiers_and_positions.insert(*output_id, (nullifier, position));
    }
}

// TODO: unify sapling and orchard leaf and retention fns
/// Calculates the sapling note commitment tree leaves and shardtree retentions for a given compact transaction
fn calculate_sapling_leaves_and_retentions<D: Domain>(
    outputs: &[CompactSaplingOutput],
    incoming_decrypted_outputs: &HashMap<OutputId, DecryptedOutput<D, ()>>,
) -> Result<Vec<(Node, Retention<BlockHeight>)>, ScanError> {
    let incoming_output_indexes = incoming_decrypted_outputs
        .keys()
        .copied()
        .map(|output_id| output_id.output_index())
        .collect::<Vec<_>>();

    if outputs.is_empty() {
        Ok(Vec::new())
    } else {
        let leaves_and_retentions = outputs
            .iter()
            .enumerate()
            .map(|(output_index, output)| {
                let note_commitment = get_compact_output_description(output)
                    .map_err(|_| ScanError::InvalidSaplingOutput)?
                    .cmu;
                let leaf = sapling_crypto::Node::from_cmu(&note_commitment);
                let decrypted: bool = incoming_output_indexes.contains(
                    &output_index
                        .try_into()
                        .expect("output indexes should be valid u32"),
                );
                let retention = if decrypted {
                    Retention::Marked
                } else {
                    Retention::Ephemeral
                };

                Ok((leaf, retention))
            })
            .collect::<Result<_, ScanError>>()?;

        Ok(leaves_and_retentions)
    }
}

// calculates the orchard note commitment tree leaves and shardtree retentions for a given compact transaction
fn calculate_orchard_leaves_and_retentions<D: Domain>(
    actions: &[CompactOrchardAction],
    incoming_decrypted_outputs: &HashMap<OutputId, DecryptedOutput<D, ()>>,
) -> Result<Vec<(MerkleHashOrchard, Retention<BlockHeight>)>, ScanError> {
    let incoming_output_indexes = incoming_decrypted_outputs
        .keys()
        .copied()
        .map(|output_id| output_id.output_index())
        .collect::<Vec<_>>();

    if actions.is_empty() {
        Ok(Vec::new())
    } else {
        let leaves_and_retentions = actions
            .iter()
            .enumerate()
            .map(|(output_index, output)| {
                let note_commitment = get_compact_action(output)
                    .map_err(|_| ScanError::InvalidOrchardAction)?
                    .cmx();
                let leaf = MerkleHashOrchard::from_cmx(&note_commitment);
                let decrypted: bool = incoming_output_indexes.contains(
                    &output_index
                        .try_into()
                        .expect("output indexes should be valid u32"),
                );
                let retention = if decrypted {
                    Retention::Marked
                } else {
                    Retention::Ephemeral
                };

                Ok((leaf, retention))
            })
            .collect::<Result<_, ScanError>>()?;

        Ok(leaves_and_retentions)
    }
}

pub(crate) async fn calculate_block_tree_bounds(
    consensus_parameters: &impl consensus::Parameters,
    fetch_request_sender: mpsc::UnboundedSender<FetchRequest>,
    compact_block: &CompactBlock,
) -> Result<TreeBounds, ServerError> {
    let (sapling_final_tree_size, orchard_final_tree_size, ironwood_final_tree_size) =
        if let Some(chain_metadata) = compact_block.chain_metadata {
            (
                chain_metadata.sapling_commitment_tree_size,
                chain_metadata.orchard_commitment_tree_size,
                chain_metadata.ironwood_commitment_tree_size,
            )
        } else {
            let sapling_activation_height = consensus_parameters
                .activation_height(consensus::NetworkUpgrade::Sapling)
                .expect("should have some sapling activation height");

            match get_compact_block_height(compact_block).cmp(&sapling_activation_height) {
                cmp::Ordering::Greater => {
                    let frontiers = client::get_frontiers(
                        fetch_request_sender.clone(),
                        get_compact_block_height(compact_block),
                    )
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
                        frontiers
                            .final_ironwood_tree()
                            .tree_size()
                            .try_into()
                            .expect("should not be more than 2^32 note commitments in the tree!"),
                    )
                }
                cmp::Ordering::Equal => (0, 0, 0),
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
    let ironwood_output_count: u32 = compact_block
        .vtx
        .iter()
        .map(|tx| tx.ironwood_actions.len())
        .sum::<usize>()
        .try_into()
        .expect("Ironwood output count cannot exceed a u32");

    Ok(TreeBounds {
        sapling_initial_tree_size: sapling_final_tree_size.saturating_sub(sapling_output_count),
        sapling_final_tree_size,
        orchard_initial_tree_size: orchard_final_tree_size.saturating_sub(orchard_output_count),
        orchard_final_tree_size,
        ironwood_initial_tree_size: ironwood_final_tree_size.saturating_sub(ironwood_output_count),
        ironwood_final_tree_size,
        provenance: TreeBoundsProvenance::Ironwood,
        revoked_testimony: RevokedTestimony::NONE,
    })
}

fn set_checkpoint_retentions<L>(
    block_height: BlockHeight,
    leaves_and_retentions: &mut [(L, Retention<BlockHeight>)],
) {
    if let Some((_leaf, retention)) = leaves_and_retentions.last_mut() {
        match retention {
            Retention::Marked => {
                *retention = Retention::Checkpoint {
                    id: block_height,
                    marking: Marking::Marked,
                };
            }
            Retention::Ephemeral => {
                *retention = Retention::Checkpoint {
                    id: block_height,
                    marking: Marking::None,
                };
            }
            // NOTE: if there are no outputs in the block, this last retention will be a checkpoint and nothing will need to be mutated.
            _ => (),
        }
    }
}

#[cfg(test)]
mod tests {
    use zingo_netutils::lightwallet_protocol::{ChainMetadata, CompactTx};

    use super::*;

    fn wallet_block_with_ironwood_size(size: u32) -> WalletBlock {
        WalletBlock {
            block_height: BlockHeight::from_u32(100),
            block_hash: BlockHash([1; 32]),
            prev_hash: BlockHash([0; 32]),
            time: 0,
            txids: vec![],
            tree_bounds: TreeBounds {
                sapling_initial_tree_size: 10,
                sapling_final_tree_size: 10,
                orchard_initial_tree_size: 10,
                orchard_final_tree_size: 10,
                ironwood_initial_tree_size: size,
                ironwood_final_tree_size: size,
                provenance: TreeBoundsProvenance::Ironwood,
                revoked_testimony: RevokedTestimony::NONE,
            },
        }
    }

    fn compact_block_with_ironwood_size(size: u32) -> CompactBlock {
        CompactBlock {
            height: 100,
            hash: vec![1; 32],
            prev_hash: vec![0; 32],
            time: 0,
            header: vec![],
            vtx: vec![],
            chain_metadata: Some(ChainMetadata {
                sapling_commitment_tree_size: 10,
                orchard_commitment_tree_size: 10,
                ironwood_commitment_tree_size: size,
            }),
        }
    }

    /// A server actively serving ironwood metadata (nonzero) that disagrees
    /// with the wallet's own nonzero calculation is corruption, not the
    /// known "server does not serve ironwood yet" case (metadata zero).
    /// Validation must reject it exactly as it does for sapling and orchard.
    #[test]
    fn nonzero_ironwood_tree_size_mismatch_is_rejected() {
        let compact_block = compact_block_with_ironwood_size(999);
        let wallet_block = wallet_block_with_ironwood_size(7);
        assert!(matches!(
            check_tree_size(&compact_block, &wallet_block),
            Err(ScanError::IncorrectTreeSize {
                shielded_protocol: PoolType::Shielded(ShieldedPool::Ironwood),
                ..
            })
        ));
    }

    /// A placeholder action that parses but decrypts to nothing: enough to
    /// be counted by the scanner without building a real note.
    fn compact_ironwood_action() -> CompactOrchardAction {
        CompactOrchardAction {
            nullifier: vec![0; 32],
            cmx: vec![0; 32],
            ephemeral_key: vec![0; 32],
            ciphertext: vec![0; 52],
        }
    }

    fn block_with_served_ironwood_actions(
        action_count: usize,
        metadata_ironwood_size: u32,
    ) -> CompactBlock {
        CompactBlock {
            height: 100,
            hash: vec![1; 32],
            prev_hash: vec![0; 32],
            time: 0,
            header: vec![],
            vtx: vec![CompactTx {
                index: 0,
                txid: vec![3; 32],
                fee: 0,
                spends: vec![],
                outputs: vec![],
                actions: vec![],
                ironwood_actions: (0..action_count)
                    .map(|_| compact_ironwood_action())
                    .collect(),
                vin: vec![],
                vout: vec![],
            }],
            chain_metadata: Some(ChainMetadata {
                sapling_commitment_tree_size: 10,
                orchard_commitment_tree_size: 10,
                ironwood_commitment_tree_size: metadata_ironwood_size,
            }),
        }
    }

    fn initial_scan_data(ironwood_initial_tree_size: u32) -> InitialScanData {
        InitialScanData {
            start_seam_block: None,
            end_seam_block: None,
            sapling_initial_tree_size: 10,
            orchard_initial_tree_size: 10,
            ironwood_initial_tree_size,
        }
    }

    /// A block's calculated tree size is the scan baseline plus every output
    /// served on the wire, for ironwood exactly as for orchard.
    #[test]
    fn ironwood_calculated_size_counts_served_actions() {
        let scan_data = scan_compact_blocks(
            vec![block_with_served_ironwood_actions(5, 105)],
            &zcash_protocol::consensus::MAIN_NETWORK,
            &HashMap::new(),
            initial_scan_data(100),
            100,
        )
        .unwrap();

        let tree_bounds = scan_data
            .wallet_blocks
            .values()
            .next()
            .unwrap()
            .tree_bounds();
        assert_eq!(tree_bounds.ironwood_initial_tree_size, 100);
        assert_eq!(tree_bounds.ironwood_final_tree_size, 105);
    }

    /// A baseline understating the true tree size below the range must fail
    /// the scan at the first served action, escaping the metadata-zero
    /// tolerance. Note positions and witnesses derive from the same
    /// baseline, so scanning past the mismatch would corrupt them silently.
    #[test]
    fn understated_ironwood_baseline_is_rejected() {
        let result = scan_compact_blocks(
            vec![block_with_served_ironwood_actions(5, 105)],
            &zcash_protocol::consensus::MAIN_NETWORK,
            &HashMap::new(),
            initial_scan_data(0),
            100,
        );

        assert!(matches!(
            result,
            Err(ScanError::IncorrectTreeSize {
                shielded_protocol: PoolType::Shielded(ShieldedPool::Ironwood),
                block_metadata_size: 105,
                calculated_size: 5,
            })
        ));
    }

    fn block_with_served_orchard_actions(
        action_count: usize,
        metadata_orchard_size: u32,
    ) -> CompactBlock {
        let mut compact_block = block_with_served_ironwood_actions(0, 10);
        compact_block.vtx[0].actions = (0..action_count)
            .map(|_| compact_ironwood_action())
            .collect();
        compact_block
            .chain_metadata
            .as_mut()
            .expect("the helper always builds metadata")
            .orchard_commitment_tree_size = metadata_orchard_size;
        compact_block
    }

    /// A wallet whose history never tracked a pool records no tree for it,
    /// so its blocks report zero where the chain reports a populated tree.
    /// That is the wallet's record failing to account for the chain, and it
    /// must be named rather than tolerated: tolerating it leaves the notes
    /// in that history undetected and the balance understating the wallet.
    #[test]
    fn untracked_pool_history_is_rejected() {
        let compact_block = compact_block_with_ironwood_size(1354);
        let wallet_block = wallet_block_with_ironwood_size(0);

        assert!(matches!(
            check_tree_size(&compact_block, &wallet_block),
            Err(ScanError::IncorrectTreeSize {
                shielded_protocol: PoolType::Shielded(ShieldedPool::Ironwood),
                block_metadata_size: 1354,
                calculated_size: 0,
            })
        ));
    }

    /// The same reasoning, for a pool that activated long before this build:
    /// nothing about it depends on which pool is newest, on an activation
    /// height, or on a network.
    #[test]
    fn untracked_pool_history_is_rejected_for_every_pool() {
        let compact_block = block_with_served_orchard_actions(0, 1354);
        let wallet_block = wallet_block_with_ironwood_size(10);

        assert!(matches!(
            check_tree_size(&compact_block, &wallet_block),
            Err(ScanError::IncorrectTreeSize {
                shielded_protocol: PoolType::Shielded(ShieldedPool::Orchard),
                block_metadata_size: 1354,
                calculated_size: 10,
            })
        ));
    }

    /// A server that does not serve a pool reports nothing for it while the
    /// block still carries that pool's outputs. Failing sync there would
    /// strand every wallet using such a server, and the wallet's own record
    /// is not what is at fault, so this is tolerated.
    #[test]
    fn a_server_not_serving_a_pool_does_not_fail_the_scan() {
        let compact_block = block_with_served_ironwood_actions(5, 0);
        let wallet_block = wallet_block_with_ironwood_size(5);

        assert!(check_tree_size(&compact_block, &wallet_block).is_ok());
    }

    /// Metadata reporting nothing where the block serves nothing either is
    /// the ordinary state of a chain below a pool's first output, and a
    /// wallet recording a tree there has a record the chain contradicts.
    #[test]
    fn a_tree_recorded_where_the_chain_has_none_is_rejected() {
        let compact_block = block_with_served_ironwood_actions(0, 0);
        let wallet_block = wallet_block_with_ironwood_size(7);

        assert!(matches!(
            check_tree_size(&compact_block, &wallet_block),
            Err(ScanError::IncorrectTreeSize {
                shielded_protocol: PoolType::Shielded(ShieldedPool::Ironwood),
                ..
            })
        ));
    }

    /// After a pool's history is reopened, the rescan is dispatched over the
    /// very blocks whose record the reopening condemned:
    /// `create_scan_task` seams each chunk on the surviving wallet block
    /// below it, and the scan trusts that seam's tree bounds over the
    /// chain metadata served with the compact blocks. A chunk seamed on a
    /// condemned block therefore reproduces the exact `IncorrectTreeSize`
    /// that caused the reopening, which reopens the history again and ends
    /// the session: whenever the reopened history spans more than one
    /// chunk, the healing loop closes without progress.
    ///
    /// The sync state below is the post-reopening contract of
    /// `reopen_scan_ranges_from` (previously scanned ranges returned to
    /// `Historic`), and the two `create_scan_task` calls are the dispatches
    /// the two concurrent scan workers receive. The assertion states the
    /// property the healing needs in order to make progress: rescanning the
    /// second chunk must not reproduce the reopening error.
    #[tokio::test]
    async fn rescan_after_reopening_must_not_reproduce_the_reopening_error() {
        use std::collections::BTreeMap;

        use crate::mocks::MockWalletBuilder;
        use crate::sync::state::create_scan_task;
        use crate::sync::{ScanPriority, ScanRange};
        use crate::wallet::SyncState;

        let regtest = zcash_protocol::local_consensus::LocalNetwork {
            overwinter: Some(BlockHeight::from_u32(1)),
            sapling: Some(BlockHeight::from_u32(1)),
            blossom: Some(BlockHeight::from_u32(1)),
            heartwood: Some(BlockHeight::from_u32(1)),
            canopy: Some(BlockHeight::from_u32(1)),
            nu5: Some(BlockHeight::from_u32(1)),
            nu6: Some(BlockHeight::from_u32(1)),
            nu6_1: Some(BlockHeight::from_u32(1)),
            nu6_2: Some(BlockHeight::from_u32(1)),
            nu6_3: Some(BlockHeight::from_u32(1)),
        };

        // A block whose ironwood record the reopening condemned and revoked:
        // by height 99 the chain holds 100 ironwood commitments, the record
        // says zero, and the strip marked that testimony revoked. Its hash
        // chains to `block_with_served_ironwood_actions` at height 100.
        let condemned_block = WalletBlock {
            block_height: BlockHeight::from_u32(99),
            block_hash: BlockHash([0; 32]),
            prev_hash: BlockHash([9; 32]),
            time: 0,
            txids: vec![],
            tree_bounds: TreeBounds {
                sapling_initial_tree_size: 10,
                sapling_final_tree_size: 10,
                orchard_initial_tree_size: 10,
                orchard_final_tree_size: 10,
                ironwood_initial_tree_size: 0,
                ironwood_final_tree_size: 0,
                provenance: TreeBoundsProvenance::Ironwood,
                revoked_testimony: RevokedTestimony {
                    sapling: false,
                    orchard: false,
                    ironwood: true,
                },
            },
        };

        let mut sync_state = SyncState::new();
        sync_state.scan_ranges = vec![
            ScanRange::from_parts(
                BlockHeight::from_u32(90)..BlockHeight::from_u32(100),
                ScanPriority::Historic,
            ),
            ScanRange::from_parts(
                BlockHeight::from_u32(100)..BlockHeight::from_u32(102),
                ScanPriority::Historic,
            ),
        ];

        let mut wallet = MockWalletBuilder::new()
            .birthday(BlockHeight::from_u32(90))
            .sync_state(sync_state)
            .wallet_blocks(BTreeMap::from([(
                BlockHeight::from_u32(99),
                condemned_block,
            )]))
            .create_mock_wallet();

        let first_chunk = create_scan_task(&regtest, &mut wallet, false)
            .unwrap()
            .expect("the lowest reopened chunk is dispatched");
        assert!(first_chunk.start_seam_block.is_none());
        let second_chunk = create_scan_task(&regtest, &mut wallet, false)
            .unwrap()
            .expect("the next chunk is dispatched while the first is still scanning");

        let (fetch_request_sender, _receiver) = mpsc::unbounded_channel();
        let initial_scan_data = InitialScanData::new(
            fetch_request_sender,
            &regtest,
            &block_with_served_ironwood_actions(5, 105),
            second_chunk.start_seam_block,
            None,
        )
        .await
        .unwrap();

        let result = scan_compact_blocks(
            vec![block_with_served_ironwood_actions(5, 105)],
            &regtest,
            &HashMap::new(),
            initial_scan_data,
            100,
        );

        if let Err(ScanError::IncorrectTreeSize {
            shielded_protocol,
            block_metadata_size,
            calculated_size,
        }) = result
        {
            panic!(
                "the reopened rescan reproduced the reopening error \
                 ({shielded_protocol} history recorded {calculated_size} where the chain \
                 reports {block_metadata_size}): reopening recreates this exact state, \
                 so the healing loop never completes"
            );
        }
    }
}

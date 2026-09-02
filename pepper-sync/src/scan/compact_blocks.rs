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
use zcash_protocol::{
    consensus::{self, BlockHeight},
    value::Zatoshis,
};
use zcash_transparent::address::Script;
use zingo_netutils::lightwallet_protocol::{
    CompactBlock, CompactOrchardAction, CompactSaplingOutput,
};
use zip32::AccountId;

use crate::{
    client::{self, FetchRequest},
    error::{ContinuityError, ScanError, ServerError},
    keys::{
        self, KeyId, ScanningKeyOps, ScanningKeys,
        transparent::{TransparentAddressId, TransparentScope},
    },
    scan::collect_outpoints_compact,
    utils::{block, get_compact_action, get_compact_output_description, transaction},
    wallet::{KeyIdInterface as _, NullifierMap, OutputId, ScanTarget, TreeBounds, WalletBlock},
    witness::WitnessData,
};

use zcash_protocol::{PoolType, ShieldedPool};

use self::runners::{DecryptedOutput, DecryptionBatchRunners};

use super::{DecryptedNoteData, InitialScanData, ScanData, collect_nullifiers_compact};

mod runners;

#[allow(clippy::complexity)]
pub(super) fn scan_compact_blocks<P>(
    compact_blocks: Vec<CompactBlock>,
    consensus_parameters: &P,
    ufvks: &HashMap<AccountId, UnifiedFullViewingKey>,
    initial_scan_data: InitialScanData,
    output_decryptions_in_batch: usize,
    transparent_inuse_addresses: HashMap<String, TransparentAddressId>,
    mut transparent_gap_addresses: HashMap<String, TransparentAddressId>,
    transparent_gap_limit: u32,
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
        output_decryptions_in_batch,
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

        let block_height = block::get_compact_height(block);
        let block_hash = block::get_compact_hash(block);

        for transaction in &block.vtx {
            let txid = transaction::get_compact_txid(transaction);

            // collect trial decryption results by transaction
            let incoming_sapling_outputs = runners.sapling.collect_results(block_hash, txid);
            let incoming_orchard_outputs = runners.orchard.collect_results(block_hash, txid);
            let incoming_ironwood_outputs = runners.ironwood.collect_results(block_hash, txid);

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

            collect_nullifiers_compact(&mut nullifiers, block_height, transaction)?;

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

            sapling_final_tree_size +=
                transaction::shielded_output_count(transaction, ShieldedPool::Sapling);
            orchard_final_tree_size +=
                transaction::shielded_output_count(transaction, ShieldedPool::Orchard);
            ironwood_final_tree_size +=
                transaction::shielded_output_count(transaction, ShieldedPool::Ironwood);

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
                block_height,
                block_hash,
                prev_hash: block::get_compact_prev_hash(block),
                time: block.time,
                txids: block
                    .vtx
                    .iter()
                    .map(transaction::get_compact_txid)
                    .collect(),
                tree_bounds: TreeBounds {
                    sapling_initial_tree_size,
                    sapling_final_tree_size,
                    orchard_initial_tree_size,
                    orchard_final_tree_size,
                    ironwood_initial_tree_size,
                    ironwood_final_tree_size,
                },
            };

            check_tree_size(block, &wallet_block)?;

            wallet_blocks.insert(wallet_block.block_height(), wallet_block);
        }
    }

    // retry transparent compact block scanning until the gap limit has been satisfied
    let mut outpoints = BTreeMap::new();
    let mut new_transparent_inuse_addresses = HashMap::new();
    'gap: loop {
        let mut gap_addresses_in_use = BTreeSet::new();

        for block in &compact_blocks {
            let block_height = block::get_compact_height(block);

            for transaction in &block.vtx {
                let txid = transaction::get_compact_txid(transaction);

                // check transparent outputs against inuse and gap addresses and add outpoints to map
                // TODO: only enable for blocks above the initial chain height when sync session started
                for output in transaction.vout.iter() {
                    let output = zcash_transparent::bundle::TxOut::new(
                        Zatoshis::from_u64(output.value)
                            .map_err(|_| ScanError::TransparentOutputInvalidValue(output.value))?,
                        Script(zcash_script::script::Code(output.script_pub_key.clone())),
                    );
                    if let Some(address) = output.recipient_address() {
                        let encoded_address =
                            keys::transparent::encode_address(consensus_parameters, address);
                        if let Some((_address, _key_id)) =
                            transparent_inuse_addresses.get_key_value(&encoded_address)
                        {
                            decrypted_scan_targets.insert(ScanTarget {
                                block_height,
                                txid,
                                narrow_scan_area: true,
                            });
                        }
                        if let Some((_address, key_id)) =
                            transparent_gap_addresses.get_key_value(&encoded_address)
                        {
                            // NOTE: the new transparent in-use addresses do not need to be appended to the transparent
                            // in-use addresses in this loop as the scan target has already been added here
                            gap_addresses_in_use.insert(*key_id);
                            decrypted_scan_targets.insert(ScanTarget {
                                block_height,
                                txid,
                                narrow_scan_area: true,
                            });
                        }
                    }
                }
                collect_outpoints_compact(&mut outpoints, block_height, transaction);
            }
        }

        if gap_addresses_in_use.is_empty() {
            break 'gap;
        }

        for (account_id, ufvk) in ufvks.iter() {
            let Some(account_pubkey) = ufvk.transparent() else {
                continue;
            };

            for scope in [
                TransparentScope::External,
                TransparentScope::Internal,
                TransparentScope::Refund,
            ] {
                // TODO: collect as nonempty?
                let gap_addresses_in_use_scoped = gap_addresses_in_use
                    .iter()
                    .filter(|id| id.account_id() == *account_id && id.scope() == scope)
                    .collect::<Vec<_>>();

                if gap_addresses_in_use_scoped.is_empty() {
                    continue;
                }

                // NOTE: the `gap_addresses_in_use` cannot be used to determine the first gap address index as there is no
                // guarantee the first gap address is in use
                let lowest_gap_address_index = transparent_gap_addresses
                .values()
                .filter(|id| id.account_id() == *account_id && id.scope() == scope)
                .map(TransparentAddressId::address_index)
                .min()
                .expect(
                    "gap addresses must exist as some are guaranteed to be in use in this scope",
                );
                let highest_gap_address_index_in_use = gap_addresses_in_use_scoped
                    .last()
                    .expect("non-empty in this scope")
                    .address_index();
                let no_of_gap_addresses_in_use = highest_gap_address_index_in_use
                    .saturating_sub(lowest_gap_address_index.index())
                    .index()
                    + 1;
                // NOTE: if we saturating add `gap_limit` to directly find the first index to derive we will not error if
                // all addresses are already in use
                let mut address_index_for_derivation = lowest_gap_address_index
                    .saturating_add(transparent_gap_limit - 1)
                    .next()
                    .ok_or_else(|| ScanError::AllAddressesInUse)?;
                let highest_address_index_for_derivation = address_index_for_derivation
                    .index()
                    .saturating_add(no_of_gap_addresses_in_use - 1);
                loop {
                    // derive new gap address for each gap address in use
                    let new_gap_address_id =
                        TransparentAddressId::new(*account_id, scope, address_index_for_derivation);
                    let new_gap_address = keys::transparent::derive_address(
                        consensus_parameters,
                        account_pubkey,
                        new_gap_address_id,
                    )
                    .map_err(ScanError::TransparentAddressDerivationError)?;
                    transparent_gap_addresses.insert(new_gap_address, new_gap_address_id);

                    // move the used gap address into inuse addresses
                    let new_inuse_address = transparent_gap_addresses
                        .iter()
                        .find(|(_address, id)| {
                            id.account_id() == *account_id
                                && id.scope() == scope
                                && id.address_index().index()
                                    == new_gap_address_id
                                        .address_index()
                                        .index()
                                        .checked_sub(transparent_gap_limit)
                                        .expect("new gap address index was derived directly from transparent gap addresses. should never underflow!")
                        })
                        .expect("new gap address index was derived directly from transparent gap addresses. should always exist!")
                        .0
                        .clone();
                    let new_inuse_address_entry = transparent_gap_addresses
                        .remove_entry(&new_inuse_address)
                        .expect("must exist in this scope!");
                    new_transparent_inuse_addresses
                        .insert(new_inuse_address_entry.0, new_inuse_address_entry.1);

                    // increment the address index until we have derived all the new gap addresses
                    if address_index_for_derivation.index() < highest_address_index_for_derivation {
                        address_index_for_derivation = address_index_for_derivation
                            .next()
                            .ok_or_else(|| ScanError::AllAddressesInUse)?;
                    } else {
                        break;
                    }
                }
            }
        }
    }

    Ok(ScanData {
        nullifiers,
        outpoints,
        wallet_blocks,
        decrypted_scan_targets,
        decrypted_note_data,
        witness_data,
        new_transparent_inuse_addresses,
        updated_transparent_gap_addresses: transparent_gap_addresses,
    })
}

fn trial_decrypt<P>(
    consensus_parameters: &P,
    scanning_keys: &ScanningKeys,
    compact_blocks: &[CompactBlock],
    output_decryptions_in_batch: usize,
) -> Result<DecryptionBatchRunners<(), (), ()>, ScanError>
where
    P: consensus::Parameters + Send + 'static,
{
    let mut runners =
        DecryptionBatchRunners::<(), (), ()>::for_keys(output_decryptions_in_batch, scanning_keys);
    for block in compact_blocks {
        runners.add_block(consensus_parameters, block.clone())?;
    }
    runners.flush();

    Ok(runners)
}

/// Checks height and hash continuity of a load of compact blocks.
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
            && block::get_compact_height(block) != prev_height + 1
        {
            return Err(ContinuityError::HeightDiscontinuity {
                height: block::get_compact_height(block),
                previous_block_height: prev_height,
            });
        }

        if let Some(prev_hash) = prev_hash
            && block::get_compact_prev_hash(block) != prev_hash
        {
            return Err(ContinuityError::HashDiscontinuity {
                height: block::get_compact_height(block),
                prev_hash: block::get_compact_prev_hash(block),
                previous_block_hash: prev_hash,
            });
        }

        prev_height = Some(block::get_compact_height(block));
        prev_hash = Some(block::get_compact_hash(block));
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
/// A zero metadata size is also the protobuf default of a server that does
/// not report the pool.
fn check_tree_size(
    compact_block: &CompactBlock,
    wallet_block: &WalletBlock,
) -> Result<(), ScanError> {
    let Some(chain_metadata) = &compact_block.chain_metadata else {
        return Ok(());
    };
    let tree_bounds = wallet_block.tree_bounds();

    for (pool, metadata_size, calculated_size) in [
        (
            ShieldedPool::Sapling,
            chain_metadata.sapling_commitment_tree_size,
            tree_bounds.sapling_final_tree_size,
        ),
        (
            ShieldedPool::Orchard,
            chain_metadata.orchard_commitment_tree_size,
            tree_bounds.orchard_final_tree_size,
        ),
        (
            ShieldedPool::Ironwood,
            chain_metadata.ironwood_commitment_tree_size,
            tree_bounds.ironwood_final_tree_size,
        ),
    ] {
        if metadata_size == calculated_size {
            continue;
        }

        if metadata_size == 0 {
            tracing::warn!(
                "{pool:?} chain metadata reports no tree size at block {} against a wallet size \
                 of {calculated_size}: either this server does not report the {pool:?} tree size, \
                 or the wallet's record overstates a pool the chain holds nothing of. The next \
                 block with a reported size decides.",
                wallet_block.block_height(),
            );
            continue;
        }

        return Err(ScanError::IncorrectTreeSize {
            shielded_protocol: PoolType::Shielded(pool),
            height: wallet_block.block_height(),
            block_metadata_size: metadata_size,
            calculated_size,
        });
    }

    Ok(())
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

            match block::get_compact_height(compact_block).cmp(&sapling_activation_height) {
                cmp::Ordering::Greater => {
                    let frontiers = client::get_frontiers(
                        fetch_request_sender.clone(),
                        block::get_compact_height(compact_block),
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

    let sapling_output_count = block::shielded_output_count(compact_block, ShieldedPool::Sapling);
    let orchard_output_count = block::shielded_output_count(compact_block, ShieldedPool::Orchard);
    let ironwood_output_count = block::shielded_output_count(compact_block, ShieldedPool::Ironwood);

    Ok(TreeBounds {
        sapling_initial_tree_size: sapling_final_tree_size.saturating_sub(sapling_output_count),
        sapling_final_tree_size,
        orchard_initial_tree_size: orchard_final_tree_size.saturating_sub(orchard_output_count),
        orchard_final_tree_size,
        ironwood_initial_tree_size: ironwood_final_tree_size.saturating_sub(ironwood_output_count),
        ironwood_final_tree_size,
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
    /// known "server does not report the ironwood tree size" case (metadata
    /// zero).
    /// Validation must reject it exactly as it does for sapling and orchard.
    #[test]
    fn nonzero_ironwood_tree_size_mismatch_is_rejected() {
        let compact_block = compact_block_with_ironwood_size(999);
        let wallet_block = wallet_block_with_ironwood_size(7);
        assert!(matches!(
            check_tree_size(&compact_block, &wallet_block),
            Err(ScanError::IncorrectTreeSize {
                height: _,
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
            HashMap::new(),
            HashMap::new(),
            10,
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
            HashMap::new(),
            HashMap::new(),
            10,
        );

        assert!(matches!(
            result,
            Err(ScanError::IncorrectTreeSize {
                height: _,
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
                height: _,
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
                height: _,
                shielded_protocol: PoolType::Shielded(ShieldedPool::Orchard),
                block_metadata_size: 1354,
                calculated_size: 10,
            })
        ));
    }

    /// A server that does not report a pool's tree size leaves it at zero
    /// while the block still carries that pool's outputs. Failing sync there
    /// would strand every wallet using such a server, and the wallet's own
    /// record is not what is at fault, so this is tolerated.
    #[test]
    fn an_unreported_tree_size_does_not_fail_the_scan() {
        let compact_block = block_with_served_ironwood_actions(5, 0);
        let wallet_block = wallet_block_with_ironwood_size(5);

        assert!(check_tree_size(&compact_block, &wallet_block).is_ok());
    }

    /// The wallet's count is cumulative, so against a non-reporting server
    /// the mismatch persists onto blocks that serve no outputs of their
    /// own; rejecting those would loop reopen-and-rescan forever.
    #[test]
    fn an_unreported_tree_size_is_tolerated_on_blocks_without_outputs() {
        let compact_block = block_with_served_ironwood_actions(0, 0);
        let wallet_block = wallet_block_with_ironwood_size(7);

        assert!(check_tree_size(&compact_block, &wallet_block).is_ok());
    }
}

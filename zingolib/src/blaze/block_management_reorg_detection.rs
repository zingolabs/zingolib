use crate::wallet::traits::FromCommitment;
use crate::wallet::{
    data::{BlockData, PoolNullifier},
    notes::ShieldedNoteInterface,
    traits::DomainWalletExt,
    tx_map::TxMap,
};
use incrementalmerkletree::frontier::CommitmentTree;
use incrementalmerkletree::{frontier, witness::IncrementalWitness, Hashable};
use orchard::{note_encryption::OrchardDomain, tree::MerkleHashOrchard};
use sapling_crypto::note_encryption::SaplingDomain;
use zcash_client_backend::proto::compact_formats::{CompactBlock, CompactTx};
use zcash_client_backend::proto::service::TreeState;
use zcash_note_encryption::Domain;

use futures::future::join_all;
use http::Uri;
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{
        mpsc::{self, UnboundedReceiver, UnboundedSender},
        RwLock,
    },
    task::{yield_now, JoinHandle},
    time::sleep,
};
use zcash_primitives::{
    consensus::BlockHeight,
    merkle_tree::{read_commitment_tree, write_commitment_tree, HashSer},
};

use super::sync_status::BatchSyncStatus;

type Node<D> = <<D as DomainWalletExt>::WalletNote as ShieldedNoteInterface>::Node;

const ORCHARD_START: &str = "000000";
/// The data relating to the blocks in the current batch
pub struct BlockManagementData {
    // List of all downloaded Compact Blocks in the current batch and
    // their hashes/commitment trees. Stored with the tallest
    // block first, and the shortest last.
    blocks_in_current_batch: Arc<RwLock<Vec<BlockData>>>,

    // List of existing blocks in the wallet. Used for reorgs
    existing_blocks: Arc<RwLock<Vec<BlockData>>>,

    // List of sapling tree states that were fetched from the server,
    // which need to be verified before we return from the function.
    pub unverified_treestates: Arc<RwLock<Vec<TreeState>>>,

    // How many blocks to process at a time.
    batch_size: u64,

    // Highest verified trees
    // The incorrect type name "TreeState" is encoded in protobuf
    // there are 'treestate' members of this type for each commitment
    // tree producing protocol
    highest_verified_trees: Option<TreeState>,

    // Link to the syncstatus where we can update progress
    pub sync_status: Arc<RwLock<BatchSyncStatus>>,
}

impl BlockManagementData {
    pub fn new(sync_status: Arc<RwLock<BatchSyncStatus>>) -> Self {
        Self {
            blocks_in_current_batch: Arc::new(RwLock::new(vec![])),
            existing_blocks: Arc::new(RwLock::new(vec![])),
            unverified_treestates: Arc::new(RwLock::new(vec![])),
            batch_size: crate::config::BATCH_SIZE,
            highest_verified_trees: None,
            sync_status,
        }
    }

    #[cfg(test)]
    pub fn new_with_batchsize(batch_size: u64) -> Self {
        let mut s = Self::new(Arc::new(RwLock::new(BatchSyncStatus::default())));
        s.batch_size = batch_size;

        s
    }

    pub async fn setup_sync(
        &mut self,
        existing_blocks: Vec<BlockData>,
        verified_tree: Option<TreeState>,
    ) {
        if !existing_blocks.is_empty()
            && existing_blocks.first().unwrap().height < existing_blocks.last().unwrap().height
        {
            panic!("Blocks are in wrong order");
        }
        self.unverified_treestates.write().await.clear();
        self.highest_verified_trees = verified_tree;

        self.blocks_in_current_batch.write().await.clear();

        self.existing_blocks.write().await.clear();
        self.existing_blocks.write().await.extend(existing_blocks);
    }

    /// Finish up the sync. This method will delete all the elements
    /// in the existing_blocks, and return up to `num` blocks and optionally
    /// formerly "existing"_blocks if `num` is large enough.
    /// Examples:
    ///   self.blocks.len() >= num all existing_blocks are discarded
    ///   self.blocks.len() < num the new self.blocks is:
    ///    self.blocks_original + ((the `num` - self.blocks_original.len()) highest
    ///     self.existing_blocks.
    pub async fn drain_existingblocks_into_blocks_with_truncation(
        &self,
        num: usize,
    ) -> Vec<BlockData> {
        self.unverified_treestates.write().await.clear();

        {
            let mut blocks = self.blocks_in_current_batch.write().await;
            blocks.extend(self.existing_blocks.write().await.drain(..));

            blocks.truncate(num);
            blocks.to_vec()
        }
    }

    pub async fn get_compact_transaction_for_nullifier_at_height(
        &self,
        nullifier: &PoolNullifier,
        height: u64,
    ) -> (CompactTx, u32) {
        self.wait_for_block(height).await;

        let cb = {
            let blocks = self.blocks_in_current_batch.read().await;
            let pos = blocks.first().unwrap().height - height;
            let bd = blocks.get(pos as usize).unwrap();

            bd.cb()
        };
        match nullifier {
            PoolNullifier::Sapling(nullifier) => {
                for compact_transaction in &cb.vtx {
                    for cs in &compact_transaction.spends {
                        if cs.nf == nullifier.to_vec() {
                            return (compact_transaction.clone(), cb.time);
                        }
                    }
                }
            }
            PoolNullifier::Orchard(nullifier) => {
                for compact_transaction in &cb.vtx {
                    for ca in &compact_transaction.actions {
                        if ca.nullifier == nullifier.to_bytes().to_vec() {
                            return (compact_transaction.clone(), cb.time);
                        }
                    }
                }
            }
        }
        panic!("Tx not found");
    }

    // Verify all the downloaded tree states
    pub(crate) async fn verify_trees(&self) -> (bool, Option<TreeState>) {
        // Verify only on the last batch
        {
            let sync_status = self.sync_status.read().await;
            if sync_status.batch_num + 1 != sync_status.batch_total {
                return (true, None);
            }
        }

        // If there's nothing to verify, return
        if self.unverified_treestates.read().await.is_empty() {
            log::debug!("nothing to verify, returning");
            return (true, None);
        }

        // Sort and de-dup the verification list
        // split_off(0) is a hack to assign ownership of the entire vector to
        // the ident on the left
        let mut unverified_tree_states: Vec<TreeState> =
            self.unverified_treestates.write().await.drain(..).collect();
        unverified_tree_states.sort_unstable_by_key(|treestate| treestate.height);
        unverified_tree_states.dedup_by_key(|treestate| treestate.height);

        // Remember the highest tree that will be verified, and return that.
        let highest_tree = unverified_tree_states.last().cloned();

        let mut start_trees = vec![];

        // Add all the verification trees as verified, so they can be used as starting points.
        // If any of them fails to verify, then we will fail the whole thing anyway.
        start_trees.extend(unverified_tree_states.iter().cloned());

        // Also add the wallet's highest tree
        if self.highest_verified_trees.is_some() {
            start_trees.push(self.highest_verified_trees.as_ref().unwrap().clone());
        }

        // If there are no available start trees, there is nothing to verify.
        if start_trees.is_empty() {
            return (true, None);
        }

        // sort
        start_trees.sort_unstable_by_key(|trees_state| trees_state.height);

        // Now, for each tree state that we need to verify, find the closest one
        let tree_pairs = unverified_tree_states
            .into_iter()
            .filter_map(|candidate| {
                let closest_tree = start_trees.iter().fold(None, |closest, start| {
                    if start.height < candidate.height {
                        Some(start)
                    } else {
                        closest
                    }
                });

                closest_tree.map(|t| (candidate, t.clone()))
            })
            .collect::<Vec<_>>();

        // Verify each tree pair
        let blocks = self.blocks_in_current_batch.clone();
        let handles: Vec<JoinHandle<bool>> = tree_pairs
            .into_iter()
            .map(|(candidate, closest)| Self::verify_tree_pair(blocks.clone(), candidate, closest))
            .collect();

        let results = join_all(handles)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>();
        // If errored out, return false
        if results.is_err() {
            return (false, None);
        }

        // If any one was false, return false
        if !results.unwrap().into_iter().all(std::convert::identity) {
            return (false, None);
        }

        (true, highest_tree)
    }

    ///  This associated fn compares a pair of trees and informs the caller
    ///  whether the candidate tree can be derived from the trusted reference
    ///  tree according to (we hope) the protocol.
    fn verify_tree_pair(
        blocks: Arc<RwLock<Vec<BlockData>>>,
        unverified_tree: TreeState,
        closest_lower_verified_tree: TreeState,
    ) -> JoinHandle<bool> {
        assert!(closest_lower_verified_tree.height <= unverified_tree.height);
        tokio::spawn(async move {
            if closest_lower_verified_tree.height == unverified_tree.height {
                return true;
            }
            let mut sapling_tree = read_commitment_tree(
                &hex::decode(closest_lower_verified_tree.sapling_tree).unwrap()[..],
            )
            .unwrap();
            let mut orchard_tree = read_commitment_tree(
                &hex::decode(closest_lower_verified_tree.orchard_tree).unwrap()[..],
            )
            .or_else(|error| match error.kind() {
                std::io::ErrorKind::UnexpectedEof => Ok(frontier::CommitmentTree::empty()),
                _ => Err(error),
            })
            .expect("Invalid orchard tree!");

            {
                let blocks = blocks.read().await;
                let top_block = blocks.first().unwrap().height;
                let start_pos = (top_block - closest_lower_verified_tree.height - 1) as usize;
                let end_pos = (top_block - unverified_tree.height) as usize;

                if start_pos >= blocks.len() || end_pos >= blocks.len() {
                    // Blocks are not in the current sync, which means this has already been verified
                    return true;
                }

                for i in (end_pos..start_pos + 1).rev() {
                    let cb = &blocks.get(i).unwrap().cb();
                    for compact_transaction in &cb.vtx {
                        update_trees_with_compact_transaction(
                            &mut sapling_tree,
                            &mut orchard_tree,
                            compact_transaction,
                        )
                    }
                }
            }
            // Verify that the verification_tree can be calculated from the start tree
            let mut sapling_buf = vec![];
            write_commitment_tree(&sapling_tree, &mut sapling_buf).unwrap();
            let mut orchard_buf = vec![];
            write_commitment_tree(&orchard_tree, &mut orchard_buf).unwrap();
            let determined_orchard_tree = hex::encode(orchard_buf);

            // Return if verified
            if (hex::encode(sapling_buf) == unverified_tree.sapling_tree)
                && (determined_orchard_tree == unverified_tree.orchard_tree)
            {
                true
            } else {
                // Parentless trees are encoded differently by zcashd for Orchard than for Sapling
                // this hack allows the case of a parentless orchard_tree
                is_orchard_tree_verified(determined_orchard_tree, unverified_tree)
            }
        })
    }
    // Invalidate the block (and wallet transactions associated with it) at the given block height
    pub async fn invalidate_block(
        reorg_height: u64,
        existing_blocks: Arc<RwLock<Vec<BlockData>>>,
        transaction_metadata_set: Arc<RwLock<TxMap>>,
    ) {
        let mut existing_blocks_writelock = existing_blocks.write().await;
        if existing_blocks_writelock.len() != 0 {
            // First, pop the first block (which is the top block) in the existing_blocks.
            let top_wallet_block = existing_blocks_writelock
                .drain(0..1)
                .next()
                .expect("there to be blocks");
            if top_wallet_block.height != reorg_height {
                panic!("Wrong block reorg'd");
            }

            // Remove all wallet transactions at the height
            transaction_metadata_set
                .write()
                .await
                .invalidate_all_transactions_after_or_at_height(reorg_height);
        }
    }

    /// Ingest the incoming blocks, handle any reorgs, then populate the block data
    pub(crate) async fn handle_reorgs_and_populate_block_mangement_data(
        &self,
        start_block: u64,
        end_block: u64,
        transaction_metadata_set: Arc<RwLock<TxMap>>,
        reorg_transmitter: UnboundedSender<Option<u64>>,
    ) -> (
        JoinHandle<Result<Option<u64>, String>>,
        UnboundedSender<CompactBlock>,
    ) {
        let (reorg_managment_thread_data, transmitter) =
            self.setup_for_threadspawn(start_block, end_block).await;

        // Handle 0:
        // Process the incoming compact blocks, collect them into `BlockData` and
        // pass them on for further processing.
        let h0: JoinHandle<Result<Option<u64>, String>> = tokio::spawn(
            reorg_managment_thread_data
                .handle_reorgs_populate_data_inner(transaction_metadata_set, reorg_transmitter),
        );

        // Handle: Final
        // Join all the handles
        let h = tokio::spawn(async move {
            let earliest_block = h0
                .await
                .map_err(|e| format!("Error processing blocks: {}", e))??;

            // Return the earliest block that was synced, accounting for all reorgs
            Ok(earliest_block)
        });

        (h, transmitter)
    }

    /// The stuff we need to do before spawning a new thread to do the bulk of the work.
    /// Clone our ARCs, update sync status, etc.
    async fn setup_for_threadspawn(
        &self,
        start_block: u64,
        end_block: u64,
    ) -> (BlockManagementThreadData, UnboundedSender<CompactBlock>) {
        // Create a new channel where we'll receive the blocks
        let (transmitter, receiver) = mpsc::unbounded_channel::<CompactBlock>();

        let blocks_in_current_batch = self.blocks_in_current_batch.clone();
        let existing_blocks = self.existing_blocks.clone();

        let sync_status = self.sync_status.clone();
        sync_status.write().await.blocks_total = start_block - end_block + 1;
        (
            BlockManagementThreadData {
                blocks_in_current_batch,
                existing_blocks,
                sync_status,
                receiver,
                // Needed as we can't borrow self in the thread...as the thread could outlive self
                batch_size: self.batch_size,
                end_block,
            },
            transmitter,
        )
    }

    async fn wait_for_first_block(&self) -> u64 {
        loop {
            match self.blocks_in_current_batch.read().await.first() {
                None => {
                    yield_now().await;
                    sleep(Duration::from_millis(100)).await;
                }
                Some(blocks) => return blocks.height,
            }
        }
    }

    /// This fn waits for some other actor to decrease the height of the
    /// last block, until it crosses the threshold of the `threshold_height` parameter.
    /// That is:
    ///   `self.blocks.read().await.last().unwrap().height`
    /// _must_ eventually be less that height for this fn to return.
    async fn wait_for_block(&self, threshold_height: u64) {
        self.wait_for_first_block().await;

        while self
            .blocks_in_current_batch
            .read()
            .await
            .last()
            .unwrap()
            .height
            > threshold_height
        {
            yield_now().await;
            sleep(Duration::from_millis(100)).await;

            // info!(
            //     "Waiting for block {}, current at {}",
            //     height,
            //     self.blocks.read().await.last().unwrap().height
            // );
        }
    }

    pub(crate) async fn is_nf_spent(&self, nf: PoolNullifier, after_height: u64) -> Option<u64> {
        self.wait_for_block(after_height).await;

        {
            // Read Lock for the Block Cache
            let blocks = self.blocks_in_current_batch.read().await;
            // take the height of the first/highest block in the batch and subtract target height to get index into the block list
            let pos = blocks.first().unwrap().height - after_height;
            match nf {
                PoolNullifier::Sapling(nf) => {
                    let nf = nf.to_vec();

                    // starting with a block and searching until the top of the batch
                    //     if this block contains a transaction
                    //         that contains a spent
                    //             with the given nullifier
                    //                 return its height
                    for i in (0..pos + 1).rev() {
                        let cb = &blocks.get(i as usize).unwrap().cb();
                        for compact_transaction in &cb.vtx {
                            for cs in &compact_transaction.spends {
                                if cs.nf == nf {
                                    return Some(cb.height);
                                }
                            }
                        }
                    }
                }
                PoolNullifier::Orchard(nf) => {
                    let nf = nf.to_bytes();

                    for i in (0..pos + 1).rev() {
                        let cb = &blocks.get(i as usize).unwrap().cb();
                        for compact_transaction in &cb.vtx {
                            for ca in &compact_transaction.actions {
                                if ca.nullifier == nf {
                                    return Some(cb.height);
                                }
                            }
                        }
                    }
                }
            }
            None
        }
    }

    pub async fn get_block_timestamp(&self, height: &BlockHeight) -> u32 {
        let height = u64::from(*height);
        self.wait_for_block(height).await;

        {
            let blocks = self.blocks_in_current_batch.read().await;
            let pos = blocks.first().unwrap().height - height;
            blocks.get(pos as usize).unwrap().cb().time
        }
    }
    /// This function handles Orchard and Sapling domains.
    /// This function takes data from the light server and uses it to construct a witness locally.  should?
    /// currently of the opinion that this function should be factored into separate concerns.
    pub(crate) async fn get_note_witness<D: DomainWalletExt>(
        &self,
        lightwalletd_uri: Uri,
        height: BlockHeight,
        transaction_num: usize,
        output_num: usize,
        activation_height: u64,
    ) -> Result<IncrementalWitness<<D::WalletNote as ShieldedNoteInterface>::Node, 32>, String>
    {
        // Get the previous block's height, because that block's commitment trees are the states at the start
        // of the requested block.
        let prev_height = { u64::from(height) - 1 };

        let (cb, mut tree) = {
            // In the edge case of a transition to a new network epoch, there is no previous tree.
            let tree = if prev_height < activation_height {
                frontier::CommitmentTree::<<D::WalletNote as ShieldedNoteInterface>::Node, 32>::empty()
            } else {
                let tree_state =
                    crate::grpc_connector::get_trees(lightwalletd_uri, prev_height).await?;
                let tree = hex::decode(D::get_tree(&tree_state)).unwrap();
                self.unverified_treestates.write().await.push(tree_state);
                read_commitment_tree(&tree[..]).map_err(|e| format!("{}", e))?
            };

            // Get the compact block for the supplied height
            let cb = {
                let height = u64::from(height);
                self.wait_for_block(height).await;

                {
                    let blocks = RwLock::read(&self.blocks_in_current_batch).await;

                    let pos = blocks.first().unwrap().height - height;
                    let bd = blocks.get(pos as usize).unwrap();

                    bd.cb()
                }
            };

            (cb, tree)
        };

        // Go over all the outputs. Remember that all the numbers are inclusive,
        // i.e., we have to scan upto and including block_height,
        // transaction_num and output_num
        for (t_num, compact_transaction) in cb.vtx.iter().enumerate() {
            use crate::wallet::traits::CompactOutput as _;
            for (o_num, compactoutput) in
                D::CompactOutput::from_compact_transaction(compact_transaction)
                    .iter()
                    .enumerate()
            {
                if let Some(node) = <D::WalletNote as ShieldedNoteInterface>::Node::from_commitment(
                    compactoutput.cmstar(),
                )
                .into()
                {
                    tree.append(node).unwrap();
                    if t_num == transaction_num && o_num == output_num {
                        return Ok(IncrementalWitness::from_tree(tree));
                    }
                }
            }
        }

        Err("Not found!".to_string())
    }
}

/// The components of a BlockMangementData we need to pass to the worker thread
struct BlockManagementThreadData {
    /// List of all downloaded blocks in the current batch and
    /// their hashes/commitment trees. Stored with the tallest
    /// block first, and the shortest last.
    blocks_in_current_batch: Arc<RwLock<Vec<BlockData>>>,
    /// List of existing blocks in the wallet. Used for reorgs
    existing_blocks: Arc<RwLock<Vec<BlockData>>>,
    /// Link to the syncstatus where we can update progress
    sync_status: Arc<RwLock<BatchSyncStatus>>,
    receiver: UnboundedReceiver<CompactBlock>,
    batch_size: u64,
    end_block: u64,
}

impl BlockManagementThreadData {
    async fn handle_reorgs_populate_data_inner(
        mut self,
        transaction_metadata_set: Arc<RwLock<TxMap>>,
        reorg_transmitter: UnboundedSender<Option<u64>>,
    ) -> Result<Option<u64>, String> {
        // Temporary holding place for blocks while we process them.
        let mut unprocessed_blocks = vec![];
        let mut earliest_block_height = None;

        // Reorg stuff
        let mut last_block_expecting = self.end_block;

        while let Some(compact_block) = self.receiver.recv().await {
            if compact_block.height % self.batch_size == 0 && !unprocessed_blocks.is_empty() {
                // Add these blocks to the list
                self.sync_status.write().await.blocks_done += unprocessed_blocks.len() as u64;
                self.blocks_in_current_batch
                    .write()
                    .await
                    .append(&mut unprocessed_blocks);
            }

            // Check if this is the last block we are expecting
            if compact_block.height == last_block_expecting {
                // Check to see if the prev block's hash matches, and if it does, finish the task
                let reorg_block = match self.existing_blocks.read().await.first() {
                    Some(top_block) => {
                        if top_block.hash() == compact_block.prev_hash().to_string() {
                            None
                        } else {
                            // send a reorg signal
                            Some(top_block.height)
                        }
                    }
                    None => {
                        // There is no top wallet block, so we can't really check for reorgs.
                        None
                    }
                };

                // If there was a reorg, then we need to invalidate the block and its associated transactions
                if let Some(reorg_height) = reorg_block {
                    BlockManagementData::invalidate_block(
                        reorg_height,
                        self.existing_blocks.clone(),
                        transaction_metadata_set.clone(),
                    )
                    .await;
                    last_block_expecting = reorg_height;
                }
                reorg_transmitter.send(reorg_block).unwrap();
            }

            earliest_block_height = Some(compact_block.height);
            unprocessed_blocks.push(BlockData::new(compact_block));
        }

        if !unprocessed_blocks.is_empty() {
            self.sync_status.write().await.blocks_done += unprocessed_blocks.len() as u64;
            self.blocks_in_current_batch
                .write()
                .await
                .append(&mut unprocessed_blocks);
        }

        Ok(earliest_block_height)
    }
}

fn is_orchard_tree_verified(determined_orchard_tree: String, unverified_tree: TreeState) -> bool {
    determined_orchard_tree == ORCHARD_START
        || determined_orchard_tree[..132] == unverified_tree.orchard_tree[..132]
            && &determined_orchard_tree[132..] == "00"
            && unverified_tree.orchard_tree[134..]
                .chars()
                .all(|character| character == '0')
}

/// The sapling tree and orchard tree for a given block height, with the block hash.
/// The hash is likely used for reorgs.
#[derive(Debug, Clone)]
pub struct CommitmentTreesForBlock {
    pub block_height: u64,
    pub block_hash: String,
    // Type alias, sapling equivalent to the type manually written out for orchard
    pub sapling_tree: sapling_crypto::CommitmentTree,
    pub orchard_tree: frontier::CommitmentTree<MerkleHashOrchard, 32>,
}

// The following four allow(unused) functions are currently only called in test code
impl CommitmentTreesForBlock {
    #[allow(unused)]
    pub fn to_tree_state(&self) -> TreeState {
        TreeState {
            height: self.block_height,
            hash: self.block_hash.clone(),
            sapling_tree: tree_to_string(&self.sapling_tree),
            orchard_tree: tree_to_string(&self.orchard_tree),
            // We don't care about the rest of these fields
            ..Default::default()
        }
    }
    #[allow(unused)]
    pub fn empty() -> Self {
        Self {
            block_height: 0,
            block_hash: "".to_string(),
            sapling_tree: CommitmentTree::empty(),
            orchard_tree: CommitmentTree::empty(),
        }
    }
    #[allow(unused)]
    pub fn from_pre_orchard_checkpoint(
        block_height: u64,
        block_hash: String,
        sapling_tree: String,
    ) -> Self {
        Self {
            block_height,
            block_hash,
            sapling_tree: read_commitment_tree(&*hex::decode(sapling_tree).unwrap()).unwrap(),
            orchard_tree: CommitmentTree::empty(),
        }
    }
}

#[allow(unused)]
pub fn tree_to_string<Node: Hashable + HashSer>(tree: &CommitmentTree<Node, 32>) -> String {
    let mut b1 = vec![];
    write_commitment_tree(tree, &mut b1).unwrap();
    hex::encode(b1)
}

pub fn update_trees_with_compact_transaction(
    sapling_tree: &mut CommitmentTree<sapling_crypto::Node, 32>,
    orchard_tree: &mut CommitmentTree<MerkleHashOrchard, 32>,
    compact_transaction: &CompactTx,
) {
    update_tree_with_compact_transaction::<SaplingDomain>(sapling_tree, compact_transaction);
    update_tree_with_compact_transaction::<OrchardDomain>(orchard_tree, compact_transaction);
}

pub fn update_tree_with_compact_transaction<D: DomainWalletExt>(
    tree: &mut CommitmentTree<Node<D>, 32>,
    compact_transaction: &CompactTx,
) where
    <D as Domain>::Recipient: crate::wallet::traits::Recipient,
    <D as Domain>::Note: PartialEq + Clone,
{
    use crate::wallet::traits::CompactOutput;
    for output in D::CompactOutput::from_compact_transaction(compact_transaction) {
        let node = Node::<D>::from_commitment(output.cmstar()).unwrap();
        tree.append(node).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use crate::{blaze::test_utils::FakeCompactBlock, wallet::data::BlockData};
    use orchard::tree::MerkleHashOrchard;
    use zcash_primitives::block::BlockHash;

    use super::*;

    fn make_fake_block_list(numblocks: u64) -> Vec<BlockData> {
        if numblocks == 0 {
            return Vec::new();
        }
        let mut prev_hash = BlockHash([0; 32]);
        let mut blocks: Vec<_> = (1..=numblocks)
            .map(|n| {
                let fake_block = FakeCompactBlock::new(n, prev_hash).block;
                prev_hash = BlockHash(fake_block.hash.clone().try_into().unwrap());
                BlockData::new(fake_block)
            })
            .collect();
        blocks.reverse();
        blocks
    }

    #[tokio::test]
    async fn verify_block_and_witness_data_blocks_order() {
        let mut scenario_bawd = BlockManagementData::new_with_batchsize(25_000);

        for numblocks in [0, 1, 2, 10] {
            let existing_blocks = make_fake_block_list(numblocks);
            scenario_bawd
                .setup_sync(existing_blocks.clone(), None)
                .await;
            assert_eq!(
                scenario_bawd.existing_blocks.read().await.len(),
                numblocks as usize
            );
            let finished_blks = scenario_bawd
                .drain_existingblocks_into_blocks_with_truncation(100)
                .await;
            assert_eq!(scenario_bawd.existing_blocks.read().await.len(), 0);
            assert_eq!(finished_blks.len(), numblocks as usize);

            for (i, finished_blk) in finished_blks.iter().enumerate() {
                assert_eq!(existing_blocks[i].hash(), finished_blk.hash());
                assert_eq!(existing_blocks[i].height, finished_blk.height);
                if i > 0 {
                    // Prove that height decreases as index increases
                    assert_eq!(finished_blk.height, finished_blks[i - 1].height - 1);
                }
            }
        }
    }

    #[tokio::test]
    async fn setup_finish_large() {
        let mut nw = BlockManagementData::new_with_batchsize(25_000);

        let existing_blocks = make_fake_block_list(200);
        nw.setup_sync(existing_blocks.clone(), None).await;
        let finished_blks = nw
            .drain_existingblocks_into_blocks_with_truncation(100)
            .await;

        assert_eq!(finished_blks.len(), 100);

        for (i, finished_blk) in finished_blks.into_iter().enumerate() {
            assert_eq!(existing_blocks[i].hash(), finished_blk.hash());
            assert_eq!(existing_blocks[i].height, finished_blk.height);
        }
    }

    #[tokio::test]
    async fn from_sapling_genesis() {
        let blocks = make_fake_block_list(200);

        // Blocks are in reverse order
        assert!(blocks.first().unwrap().height > blocks.last().unwrap().height);

        let start_block = blocks.first().unwrap().height;
        let end_block = blocks.last().unwrap().height;

        let sync_status = Arc::new(RwLock::new(BatchSyncStatus::default()));
        let mut nw = BlockManagementData::new(sync_status);
        nw.setup_sync(vec![], None).await;

        let (reorg_transmitter, mut reorg_receiver) = mpsc::unbounded_channel();

        let (h, cb_sender) = nw
            .handle_reorgs_and_populate_block_mangement_data(
                start_block,
                end_block,
                Arc::new(RwLock::new(TxMap::new_with_witness_trees_address_free())),
                reorg_transmitter,
            )
            .await;

        let send_h: JoinHandle<Result<(), String>> = tokio::spawn(async move {
            for block in blocks {
                cb_sender
                    .send(block.cb())
                    .map_err(|e| format!("Couldn't send block: {}", e))?;
            }
            if let Some(Some(_h)) = reorg_receiver.recv().await {
                return Err("Should not have requested a reorg!".to_string());
            }
            Ok(())
        });

        assert_eq!(h.await.unwrap().unwrap().unwrap(), end_block);

        join_all(vec![send_h])
            .await
            .into_iter()
            .collect::<Result<Result<(), String>, _>>()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn with_existing_batched() {
        let mut blocks = make_fake_block_list(200);

        // Blocks are in reverse order
        assert!(blocks.first().unwrap().height > blocks.last().unwrap().height);

        // Use the first 50 blocks as "existing", and then sync the other 150 blocks.
        let existing_blocks = blocks.split_off(150);

        let start_block = blocks.first().unwrap().height;
        let end_block = blocks.last().unwrap().height;

        let mut nw = BlockManagementData::new_with_batchsize(25);
        nw.setup_sync(existing_blocks, None).await;

        let (reorg_transmitter, mut reorg_receiver) = mpsc::unbounded_channel();

        let (h, cb_sender) = nw
            .handle_reorgs_and_populate_block_mangement_data(
                start_block,
                end_block,
                Arc::new(RwLock::new(TxMap::new_with_witness_trees_address_free())),
                reorg_transmitter,
            )
            .await;

        let send_h: JoinHandle<Result<(), String>> = tokio::spawn(async move {
            for block in blocks {
                cb_sender
                    .send(block.cb())
                    .map_err(|e| format!("Couldn't send block: {}", e))?;
            }
            if let Some(Some(_h)) = reorg_receiver.recv().await {
                return Err("Should not have requested a reorg!".to_string());
            }
            Ok(())
        });

        assert_eq!(h.await.unwrap().unwrap().unwrap(), end_block);

        join_all(vec![send_h])
            .await
            .into_iter()
            .collect::<Result<Result<(), String>, _>>()
            .unwrap()
            .unwrap();

        let finished_blks = nw
            .drain_existingblocks_into_blocks_with_truncation(100)
            .await;
        assert_eq!(finished_blks.len(), 100);
        assert_eq!(finished_blks.first().unwrap().height, start_block);
        assert_eq!(finished_blks.last().unwrap().height, start_block - 100 + 1);
    }

    #[tokio::test]
    async fn with_reorg() {
        let mut blocks = make_fake_block_list(100);

        // Blocks are in reverse order
        assert!(blocks.first().unwrap().height > blocks.last().unwrap().height);

        // Use the first 50 blocks as "existing", and then sync the other 50 blocks.
        let existing_blocks = blocks.split_off(50);

        // The first 5 blocks, blocks 46-50 will be reorg'd, so duplicate them
        let num_reorged = 5;
        let mut reorged_blocks = existing_blocks
            .iter()
            .take(num_reorged)
            .cloned()
            .collect::<Vec<_>>();

        // Reset the hashes
        for i in 0..num_reorged {
            let mut hash = [0u8; 32];
            rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut hash);

            if i == 0 {
                let mut cb = blocks.pop().unwrap().cb();
                cb.prev_hash = hash.to_vec();
                blocks.push(BlockData::new(cb));
            } else {
                let mut cb = reorged_blocks[i - 1].cb();
                cb.prev_hash = hash.to_vec();
                reorged_blocks[i - 1] = BlockData::new(cb);
            }

            let mut cb = reorged_blocks[i].cb();
            cb.hash = hash.to_vec();
            reorged_blocks[i] = BlockData::new(cb);
        }
        {
            let mut cb = reorged_blocks[4].cb();
            cb.prev_hash = existing_blocks
                .iter()
                .find(|b| b.height == 45)
                .unwrap()
                .cb()
                .hash
                .to_vec();
            reorged_blocks[4] = BlockData::new(cb);
        }

        let start_block = blocks.first().unwrap().height;
        let end_block = blocks.last().unwrap().height;

        let sync_status = Arc::new(RwLock::new(BatchSyncStatus::default()));
        let mut nw = BlockManagementData::new(sync_status);
        nw.setup_sync(existing_blocks, None).await;

        let (reorg_transmitter, mut reorg_receiver) = mpsc::unbounded_channel();

        let (h, cb_sender) = nw
            .handle_reorgs_and_populate_block_mangement_data(
                start_block,
                end_block,
                Arc::new(RwLock::new(TxMap::new_with_witness_trees_address_free())),
                reorg_transmitter,
            )
            .await;

        let send_h: JoinHandle<Result<(), String>> = tokio::spawn(async move {
            // Send the normal blocks
            for block in blocks {
                cb_sender
                    .send(block.cb())
                    .map_err(|e| format!("Couldn't send block: {}", e))?;
            }

            // Expect and send the reorg'd blocks
            let mut expecting_height = 50;
            let mut sent_ctr = 0;

            while let Some(Some(h)) = reorg_receiver.recv().await {
                assert_eq!(h, expecting_height);

                expecting_height -= 1;
                sent_ctr += 1;

                cb_sender
                    .send(reorged_blocks.drain(0..1).next().unwrap().cb())
                    .map_err(|e| format!("Couldn't send block: {}", e))?;
            }

            assert_eq!(sent_ctr, num_reorged);
            assert!(reorged_blocks.is_empty());

            Ok(())
        });

        assert_eq!(
            h.await.unwrap().unwrap().unwrap(),
            end_block - num_reorged as u64
        );

        join_all(vec![send_h])
            .await
            .into_iter()
            .collect::<Result<Result<(), String>, _>>()
            .unwrap()
            .unwrap();

        let finished_blks = nw
            .drain_existingblocks_into_blocks_with_truncation(100)
            .await;
        assert_eq!(finished_blks.len(), 100);
        assert_eq!(finished_blks.first().unwrap().height, start_block);
        assert_eq!(finished_blks.last().unwrap().height, start_block - 100 + 1);

        // Verify the hashes
        for i in 0..(finished_blks.len() - 1) {
            assert_eq!(
                finished_blks[i].cb().prev_hash,
                finished_blks[i + 1].cb().hash
            );
            assert_eq!(
                finished_blks[i].hash(),
                finished_blks[i].cb().hash().to_string()
            );
        }
    }

    #[tokio::test]
    async fn setup_finish_simple() {
        let mut nw = BlockManagementData::new_with_batchsize(25_000);

        let cb = FakeCompactBlock::new(1, BlockHash([0u8; 32])).into_cb();
        let blks = vec![BlockData::new(cb)];

        nw.setup_sync(blks.clone(), None).await;
        let finished_blks = nw.drain_existingblocks_into_blocks_with_truncation(1).await;

        assert_eq!(blks[0].hash(), finished_blks[0].hash());
        assert_eq!(blks[0].height, finished_blks[0].height);
    }

    const SAPLING_START: &str = "01f2e6144dbd45cf3faafd337fe59916fe5659b4932a4a008b535b8912a8f5c0000131ac2795ef458f5223f929680085935ebd5cb84d4849b3813b03aeb80d812241020001bcc10bd2116f34cd46d0dedef75c489d6ef9b6b551c0521e3b2e56b7f641fb01";
    const ORCHARD_START: &str = "000000";
    const SAPLING_END: &str = "01ea571adc215d4eac2925efafd854c76a8afc69f07d49155febaa46f979c6616d0003000001dc4b3f1381533d99b536252aaba09b08fa1c1ddc0687eb4ede716e6a5fbfb003";
    const ORCHARD_END: &str = "011e9dcd34e245194b66dff86b1cabaa87b29e96df711e4fff72da64e88656090001c3bf930865f0c2139ce1548b0bfabd8341943571e8af619043d84d12b51355071f00000000000000000000000000000000000000000000000000000000000000";
    const BLOCK: &str = "10071a209bf92fdc55b77cbae5d13d49b4d64374ebe7ac3c5579134100e600bc01208d052220b84a8818a26321dca19ad1007d4bb99fafa07b048a0b471b1a4bbd0199ea3c0828d2fe9299063a4612203bc5bbb7df345e336288a7ffe8fac6f7dcb4ee6e4d3b4e852bcd7d52fef8d66a2a220a20d2f3948ed5e06345446d753d2688cdb1d949e0fed1e0f271e04197d0c63251023ace0308011220d151d78dec5dcf5e0308f7bfbf00d80be27081766d2cb5070c22b4556aadc88122220a20bd5312efa676f7000e5d660f01bfd946fd11102af75e7f969a0d84931e16f3532a220a20601ed4084186c4499a1bb5000490582649da2df8f2ba3e749a07c5c0aaeb9f0e2a220a20ea571adc215d4eac2925efafd854c76a8afc69f07d49155febaa46f979c6616d329c010a2062faac36e47bf420b90379b7c96cc5956daa0deedb4ed392928ba588fa5aac1012201e9dcd34e245194b66dff86b1cabaa87b29e96df711e4fff72da64e8865609001a20f5153d2a8ee2e211a7966b52e6bb3b089f32f8c55a57df1f245e6f1ec3356b982234e26e07bfb0d3c0bae348a6196ba3f069e3a1861fa4da1895ab4ad86ece7934231f6490c6e26b63298d8464560b41b2941add03ac329c010a2028ac227306fb680dc501ba9c8c51312b29b2eb79d61e5645f5486039462a620b1220c3bf930865f0c2139ce1548b0bfabd8341943571e8af619043d84d12b51355071a200803db70afdeb80b0f47a703df68c62e007e162b8b983b6f2f5df8d7b24c18a322342b1f151f67048590976a3a69e1d8a6c7408c3489bd81696cad15a9ac0a92d20f1b17ebf717f52695aeaf123aaab0ac406d6afd4a";

    fn decode_block() -> CompactBlock {
        <CompactBlock as prost::Message>::decode(&*hex::decode(BLOCK).unwrap()).unwrap()
    }

    #[test]
    fn orchard_starts_empty() {
        let empty_tree = read_commitment_tree(&*hex::decode(ORCHARD_START).unwrap()).unwrap();
        assert_eq!(empty_tree, CommitmentTree::<MerkleHashOrchard, 32>::empty());
    }

    #[test]
    fn get_sapling_end_from_start_plus_block() {
        let mut start_tree = read_commitment_tree(&*hex::decode(SAPLING_START).unwrap()).unwrap();
        let cb = decode_block();

        for compact_transaction in &cb.vtx {
            super::update_tree_with_compact_transaction::<SaplingDomain>(
                &mut start_tree,
                compact_transaction,
            )
        }
        let end_tree = read_commitment_tree(&*hex::decode(SAPLING_END).unwrap()).unwrap();
        assert_eq!(start_tree, end_tree);
    }

    #[test]
    #[ignore]
    fn get_orchard_end_from_start_plus_block() {
        let mut start_tree = read_commitment_tree(&*hex::decode(ORCHARD_START).unwrap()).unwrap();
        let cb = decode_block();

        for compact_transaction in &cb.vtx {
            super::update_tree_with_compact_transaction::<OrchardDomain>(
                &mut start_tree,
                compact_transaction,
            )
        }
        let end_tree = read_commitment_tree(&*hex::decode(ORCHARD_END).unwrap()).unwrap();
        assert_eq!(start_tree, end_tree);
    }
}

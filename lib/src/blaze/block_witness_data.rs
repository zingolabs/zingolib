use crate::wallet::traits::Spend;
use crate::{
    compact_formats::{CompactBlock, CompactTx, TreeState},
    grpc_connector::GrpcConnector,
    lightclient::checkpoints::get_all_main_checkpoints,
    wallet::{
        data::{BlockData, ChannelNullifier, TransactionMetadata, WitnessCache},
        traits::{DomainWalletExt, FromCommitment, ReceivedNoteAndMetadata},
        transactions::TransactionMetadataSet,
    },
};
use orchard::{note_encryption::OrchardDomain, tree::MerkleHashOrchard};
use zcash_note_encryption::Domain;
use zingoconfig::{ChainType, ZingoConfig, MAX_REORG};

use futures::future::join_all;
use http::Uri;
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{
        mpsc::{self, UnboundedSender},
        RwLock,
    },
    task::{yield_now, JoinHandle},
    time::sleep,
};
use zcash_primitives::{
    consensus::{BlockHeight, NetworkUpgrade, Parameters},
    merkle_tree::{CommitmentTree, Hashable, IncrementalWitness},
    sapling::{note_encryption::SaplingDomain, Node as SaplingNode},
    transaction::TxId,
};

use super::{fixed_size_buffer::FixedSizeBuffer, sync_status::BatchSyncStatus};

type Node<D> =
    <<D as DomainWalletExt<zingoconfig::ChainType>>::WalletNote as ReceivedNoteAndMetadata>::Node;

pub struct BlockAndWitnessData {
    // List of all downloaded blocks in the current batch and
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
    sync_status: Arc<RwLock<BatchSyncStatus>>,

    sapling_activation_height: u64,
    orchard_activation_height: u64,
}

impl BlockAndWitnessData {
    pub fn new(config: &ZingoConfig, sync_status: Arc<RwLock<BatchSyncStatus>>) -> Self {
        Self {
            blocks_in_current_batch: Arc::new(RwLock::new(vec![])),
            existing_blocks: Arc::new(RwLock::new(vec![])),
            unverified_treestates: Arc::new(RwLock::new(vec![])),
            batch_size: 25,
            highest_verified_trees: None,
            sync_status,
            sapling_activation_height: config.sapling_activation_height(),
            orchard_activation_height: config
                .chain
                .activation_height(NetworkUpgrade::Nu5)
                .unwrap()
                .into(),
        }
    }

    #[cfg(test)]
    pub fn new_with_batchsize(config: &ZingoConfig, batch_size: u64) -> Self {
        let mut s = Self::new(config, Arc::new(RwLock::new(BatchSyncStatus::default())));
        s.batch_size = batch_size;

        s
    }

    pub async fn setup_sync(
        &mut self,
        existing_blocks: Vec<BlockData>,
        verified_tree: Option<TreeState>,
    ) {
        if !existing_blocks.is_empty() {
            if existing_blocks.first().unwrap().height < existing_blocks.last().unwrap().height {
                panic!("Blocks are in wrong order");
            }
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
        nullifier: &ChannelNullifier,
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
            ChannelNullifier::Sapling(nullifier) => {
                for compact_transaction in &cb.vtx {
                    for cs in &compact_transaction.spends {
                        if cs.nf == nullifier.to_vec() {
                            return (compact_transaction.clone(), cb.time);
                        }
                    }
                }
            }
            ChannelNullifier::Orchard(nullifier) => {
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
    pub async fn verify_trees(&self) -> (bool, Option<TreeState>) {
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
        let highest_tree = unverified_tree_states
            .last()
            .map(|treestate| treestate.clone());

        let mut start_trees = vec![];

        // Collect all the checkpoints
        start_trees.extend(
            get_all_main_checkpoints()
                .into_iter()
                .map(|(h, hash, tree)| {
                    let mut trees_state = TreeState::default();
                    trees_state.height = h;
                    trees_state.hash = hash.to_string();
                    trees_state.sapling_tree = tree.to_string();
                    trees_state.orchard_tree = String::from("000000");

                    trees_state
                }),
        );

        // Add all the verification trees as verified, so they can be used as starting points.
        // If any of them fails to verify, then we will fail the whole thing anyway.
        start_trees.extend(unverified_tree_states.iter().map(|t| t.clone()));

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

                if closest_tree.is_some() {
                    Some((candidate, closest_tree.unwrap().clone()))
                } else {
                    None
                }
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

        return (true, highest_tree);
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
            let mut sapling_tree = CommitmentTree::<SaplingNode>::read(
                &hex::decode(closest_lower_verified_tree.sapling_tree).unwrap()[..],
            )
            .unwrap();
            let mut orchard_tree = CommitmentTree::<MerkleHashOrchard>::read(
                &hex::decode(closest_lower_verified_tree.orchard_tree).unwrap()[..],
            )
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
                    let cb = &blocks.get(i as usize).unwrap().cb();
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
            sapling_tree.write(&mut sapling_buf).unwrap();
            let mut orchard_buf = vec![];
            orchard_tree.write(&mut orchard_buf).unwrap();
            let determined_orchard_tree = hex::encode(orchard_buf);

            // Return if verified
            if (hex::encode(sapling_buf) == unverified_tree.sapling_tree)
                && (determined_orchard_tree == unverified_tree.orchard_tree)
            {
                true
            } else {
                // Parentless trees are encoded differently by zcashd for Orchard than for Sapling
                // this hack allows the case of a parentless orchard_tree
                if determined_orchard_tree[..132] == unverified_tree.orchard_tree[..132]
                    && &determined_orchard_tree[132..] == "00"
                    && unverified_tree.orchard_tree[134..]
                        .chars()
                        .all(|character| character == '0')
                {
                    true
                } else {
                    dbg!(determined_orchard_tree, unverified_tree.orchard_tree);
                    false
                }
            }
        })
    }
    // Invalidate the block (and wallet transactions associated with it) at the given block height
    pub async fn invalidate_block(
        reorg_height: u64,
        existing_blocks: Arc<RwLock<Vec<BlockData>>>,
        transaction_metadata_set: Arc<RwLock<TransactionMetadataSet>>,
    ) {
        // First, pop the first block (which is the top block) in the existing_blocks.
        let top_wallet_block = existing_blocks.write().await.drain(0..1).next().unwrap();
        if top_wallet_block.height != reorg_height {
            panic!("Wrong block reorg'd");
        }

        // Remove all wallet transactions at the height
        transaction_metadata_set
            .write()
            .await
            .remove_txns_at_height(reorg_height);
    }

    /// Start a new sync where we ingest all the blocks
    pub async fn start(
        &self,
        start_block: u64,
        end_block: u64,
        transaction_metadata_set: Arc<RwLock<TransactionMetadataSet>>,
        reorg_transmitter: UnboundedSender<Option<u64>>,
    ) -> (
        JoinHandle<Result<u64, String>>,
        UnboundedSender<CompactBlock>,
    ) {
        //info!("Starting node and witness sync");
        let batch_size = self.batch_size;

        // Create a new channel where we'll receive the blocks
        let (transmitter, mut receiver) = mpsc::unbounded_channel::<CompactBlock>();

        let blocks = self.blocks_in_current_batch.clone();
        let existing_blocks = self.existing_blocks.clone();

        let sync_status = self.sync_status.clone();
        sync_status.write().await.blocks_total = start_block - end_block + 1;

        // Handle 0:
        // Process the incoming compact blocks, collect them into `BlockData` and
        // pass them on for further processing.
        // We also trigger the node commitment tree update every `batch_size` blocks
        // using the Sapling tree fetched
        // from the server temporarily, but we verify it before we return it

        let h0: JoinHandle<Result<u64, String>> = tokio::spawn(async move {
            // Temporary holding place for blocks while we process them.
            let mut blks = vec![];
            let mut earliest_block_height = 0;

            // Reorg stuff
            let mut last_block_expecting = end_block;

            while let Some(compact_block) = receiver.recv().await {
                if compact_block.height % batch_size == 0 {
                    if !blks.is_empty() {
                        // Add these blocks to the list
                        sync_status.write().await.blocks_done += blks.len() as u64;
                        blocks.write().await.append(&mut blks);
                    }
                }

                // Check if this is the last block we are expecting
                if compact_block.height == last_block_expecting {
                    // Check to see if the prev block's hash matches, and if it does, finish the task
                    let reorg_block = match existing_blocks.read().await.first() {
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
                        Self::invalidate_block(
                            reorg_height,
                            existing_blocks.clone(),
                            transaction_metadata_set.clone(),
                        )
                        .await;
                        last_block_expecting = reorg_height;
                    }
                    reorg_transmitter.send(reorg_block).unwrap();
                }

                earliest_block_height = compact_block.height;
                blks.push(BlockData::new(compact_block));
            }

            if !blks.is_empty() {
                // We'll now dispatch these blocks for updating the witness
                sync_status.write().await.blocks_done += blks.len() as u64;
                blocks.write().await.append(&mut blks);
            }

            Ok(earliest_block_height)
        });

        // Handle: Final
        // Join all the handles
        let h = tokio::spawn(async move {
            let earliest_block = h0
                .await
                .map_err(|e| format!("Error processing blocks: {}", e))??;

            // Return the earlist block that was synced, accounting for all reorgs
            return Ok(earliest_block);
        });

        return (h, transmitter);
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

    pub(crate) async fn is_nf_spent(&self, nf: ChannelNullifier, after_height: u64) -> Option<u64> {
        self.wait_for_block(after_height).await;

        {
            // Read Lock
            let blocks = self.blocks_in_current_batch.read().await;
            let pos = blocks.first().unwrap().height - after_height;
            match nf {
                ChannelNullifier::Sapling(nf) => {
                    let nf = nf.to_vec();

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
                ChannelNullifier::Orchard(nf) => {
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
    /// This function takes data from the Untrusted Malicious Proxy, and uses it to construct a witness locally.  I am
    /// currently of the opinion that this function should be factored into separate concerns.
    pub(crate) async fn get_note_witness<D>(
        &self,
        uri: Uri,
        height: BlockHeight,
        transaction_num: usize,
        output_num: usize,
        activation_height: u64,
    ) -> Result<IncrementalWitness<<D::WalletNote as ReceivedNoteAndMetadata>::Node>, String>
    where
        D: DomainWalletExt<zingoconfig::ChainType>,
        D::Note: PartialEq + Clone,
        D::ExtractedCommitmentBytes: Into<[u8; 32]>,
        D::Recipient: crate::wallet::traits::Recipient,
    {
        // Get the previous block's height, because that block's commitment trees are the states at the start
        // of the requested block.
        let prev_height = { u64::from(height) - 1 };

        let (cb, mut tree) = {
            // In the edge case of a transition to a new network epoch, there is no previous tree.
            let tree = if prev_height < activation_height {
                CommitmentTree::empty()
            } else {
                let tree_state = GrpcConnector::get_trees(uri, prev_height).await?;
                let tree = hex::decode(D::get_tree(&tree_state)).unwrap();
                self.unverified_treestates.write().await.push(tree_state);
                CommitmentTree::read(&tree[..]).map_err(|e| format!("{}", e))?
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
                if let Some(node) =
                    <D::WalletNote as ReceivedNoteAndMetadata>::Node::from_commitment(
                        &compactoutput.cmstar(),
                    )
                    .into()
                {
                    tree.append(node).unwrap();
                    if t_num == transaction_num && o_num == output_num {
                        return Ok(IncrementalWitness::from_tree(&tree));
                    }
                }
            }
        }

        Err("Not found!".to_string())
    }
    #[allow(dead_code)]
    pub async fn get_sapling_note_witness(
        &self,
        uri: Uri,
        height: BlockHeight,
        transaction_num: usize,
        output_num: usize,
    ) -> Result<IncrementalWitness<SaplingNode>, String> {
        self.get_note_witness::<SaplingDomain<zingoconfig::ChainType>>(
            uri,
            height,
            transaction_num,
            output_num,
            self.sapling_activation_height,
        )
        .await
    }

    #[allow(dead_code)]
    pub async fn get_orchard_note_witness(
        &self,
        uri: Uri,
        height: BlockHeight,
        transaction_num: usize,
        action_num: usize,
    ) -> Result<IncrementalWitness<MerkleHashOrchard>, String> {
        self.get_note_witness::<OrchardDomain>(
            uri,
            height,
            transaction_num,
            action_num,
            self.orchard_activation_height,
        )
        .await
    }

    // Stream all the outputs start at the block till the highest block available.
    pub(crate) async fn update_witness_after_block<D: DomainWalletExt<zingoconfig::ChainType>>(
        &self,
        witnesses: WitnessCache<Node<D>>,
        nullifier: <D::WalletNote as ReceivedNoteAndMetadata>::Nullifier,
    ) -> WitnessCache<Node<D>>
    where
        <D as Domain>::Recipient: crate::wallet::traits::Recipient,
        <D as Domain>::Note: PartialEq + Clone,
    {
        let height = witnesses.top_height + 1;

        // Check if we've already synced all the requested blocks
        if height > self.wait_for_first_block().await {
            return witnesses;
        }
        self.wait_for_block(height).await;

        let mut fsb = FixedSizeBuffer::new(MAX_REORG);

        let top_block = {
            let mut blocks = self.blocks_in_current_batch.read().await;
            let top_block = blocks.first().unwrap().height;
            let bottom_block = blocks.last().unwrap().height;
            let pos = top_block - height;

            // Get the last witness, and then use that.
            let mut w = witnesses.last().unwrap().clone();
            witnesses.into_fsb(&mut fsb);

            for i in (0..pos + 1).rev() {
                let cb = &blocks.get(i as usize).unwrap().cb();
                for compact_transaction in &cb.vtx {
                    use crate::wallet::traits::CompactOutput as _;
                    for co in D::CompactOutput::from_compact_transaction(compact_transaction) {
                        if let Some(node) = Node::<D>::from_commitment(&co.cmstar()).into() {
                            w.append(node).unwrap();
                        }
                    }
                }

                // At the end of every block, update the witness in the array
                fsb.push(w.clone());

                if i % 250 == 0 {
                    // Every 250 blocks, give up the lock, let other threads proceed and then re-acquire it
                    drop(blocks);
                    yield_now().await;
                    blocks = self.blocks_in_current_batch.read().await;
                }
                self.sync_status.write().await.witnesses_updated.insert(
                    <D::Bundle as crate::wallet::traits::Bundle<D, ChainType>>::Spend::wallet_nullifier(
                        &nullifier,
                    ),
                    top_block - bottom_block - i,
                );
            }

            top_block
        };

        return WitnessCache::new(fsb.into_vec(), top_block);
    }

    async fn update_witness_after_reciept_to_block_end<D: DomainWalletExt<zingoconfig::ChainType>>(
        &self,
        height: &BlockHeight,
        transaction_id: &TxId,
        output_num: u32,
        witnesses: WitnessCache<<D::WalletNote as ReceivedNoteAndMetadata>::Node>,
    ) -> WitnessCache<<D::WalletNote as ReceivedNoteAndMetadata>::Node>
    where
        D::Recipient: crate::wallet::traits::Recipient,
        D::Note: PartialEq + Clone,
        <D::WalletNote as ReceivedNoteAndMetadata>::Node: FromCommitment,
    {
        let height = u64::from(*height);
        self.wait_for_block(height).await;

        // We'll update the rest of the block's witnesses here. Notice we pop the last witness, and we'll
        // add the updated one back into the array at the end of this function.
        let mut w = witnesses.last().unwrap().clone();

        {
            let blocks = self.blocks_in_current_batch.read().await;
            let pos = blocks.first().unwrap().height - height;

            let mut transaction_id_found = false;
            let mut output_found = false;

            let cb = &blocks.get(pos as usize).unwrap().cb();
            for compact_transaction in &cb.vtx {
                if !transaction_id_found
                    && TransactionMetadata::new_txid(&compact_transaction.hash) == *transaction_id
                {
                    transaction_id_found = true;
                }
                use crate::wallet::traits::CompactOutput as _;
                let outputs = D::CompactOutput::from_compact_transaction(compact_transaction);
                for j in 0..outputs.len() as u32 {
                    // If we've already passed the transaction id and output_num, stream the results
                    if transaction_id_found && output_found {
                        let compact_output = outputs.get(j as usize).unwrap();
                        let node =
                            <D::WalletNote as ReceivedNoteAndMetadata>::Node::from_commitment(
                                &compact_output.cmstar(),
                            )
                            .unwrap();
                        w.append(node).unwrap();
                    }

                    // Mark as found if we reach the transaction id and output_num. Starting with the next output,
                    // we'll stream all the data to the requester
                    if !output_found && transaction_id_found && j == output_num {
                        output_found = true;
                    }
                }
            }

            if !transaction_id_found || !output_found {
                panic!("Txid or output not found");
            }
        }

        // Replace the last witness in the vector with the newly computed one.
        WitnessCache::new(vec![w], height)
    }
    pub(crate) async fn update_witness_after_receipt<D: DomainWalletExt<zingoconfig::ChainType>>(
        &self,
        height: &BlockHeight,
        transaction_id: &TxId,
        output_num: u32,
        witnesses: WitnessCache<<D::WalletNote as ReceivedNoteAndMetadata>::Node>,
        nullifier: <D::WalletNote as ReceivedNoteAndMetadata>::Nullifier,
    ) -> WitnessCache<<D::WalletNote as ReceivedNoteAndMetadata>::Node>
    where
        D::Recipient: crate::wallet::traits::Recipient,
        D::Note: PartialEq + Clone,
        <D::WalletNote as ReceivedNoteAndMetadata>::Node: FromCommitment,
    {
        let witnesses = self
            .update_witness_after_reciept_to_block_end::<D>(
                height,
                transaction_id,
                output_num,
                witnesses,
            )
            .await;
        self.update_witness_after_block::<D>(witnesses, nullifier)
            .await
    }
}

/// The sapling tree and orchard tree for a given block height, with the block hash.
/// The hash is likely used for reorgs.
#[derive(Debug, Clone)]
pub struct CommitmentTreesForBlock {
    pub block_height: u64,
    pub block_hash: String,
    pub sapling_tree: CommitmentTree<SaplingNode>,
    pub orchard_tree: CommitmentTree<MerkleHashOrchard>,
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
            sapling_tree: CommitmentTree::read(&*hex::decode(sapling_tree).unwrap()).unwrap(),
            orchard_tree: CommitmentTree::empty(),
        }
    }
}

#[allow(unused)]
pub fn tree_to_string<Node: Hashable>(tree: &CommitmentTree<Node>) -> String {
    let mut b1 = vec![];
    tree.write(&mut b1).unwrap();
    hex::encode(b1)
}

pub fn update_trees_with_compact_transaction(
    sapling_tree: &mut CommitmentTree<SaplingNode>,
    orchard_tree: &mut CommitmentTree<MerkleHashOrchard>,
    compact_transaction: &CompactTx,
) {
    update_tree_with_compact_transaction::<SaplingDomain<ChainType>>(
        sapling_tree,
        compact_transaction,
    );
    update_tree_with_compact_transaction::<OrchardDomain>(orchard_tree, compact_transaction);
}

pub fn update_tree_with_compact_transaction<D: DomainWalletExt<ChainType>>(
    tree: &mut CommitmentTree<Node<D>>,
    compact_transaction: &CompactTx,
) where
    <D as Domain>::Recipient: crate::wallet::traits::Recipient,
    <D as Domain>::Note: PartialEq + Clone,
{
    use crate::wallet::traits::CompactOutput;
    for output in D::CompactOutput::from_compact_transaction(&compact_transaction) {
        let node = Node::<D>::from_commitment(output.cmstar()).unwrap();
        tree.append(node).unwrap()
    }
}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use crate::blaze::sync_status::BatchSyncStatus;
    use crate::compact_formats::CompactBlock;
    use crate::wallet::transactions::TransactionMetadataSet;
    use crate::{
        blaze::test_utils::{FakeCompactBlock, FakeCompactBlockList},
        wallet::data::BlockData,
    };
    use futures::future::join_all;
    use orchard::tree::MerkleHashOrchard;
    use rand::rngs::OsRng;
    use rand::RngCore;
    use tokio::sync::RwLock;
    use tokio::{sync::mpsc::unbounded_channel, task::JoinHandle};
    use zcash_primitives::block::BlockHash;
    use zcash_primitives::merkle_tree::CommitmentTree;
    use zcash_primitives::sapling;
    use zingoconfig::{ChainType, ZingoConfig};

    use super::*;

    #[tokio::test]
    async fn setup_finish_simple() {
        let mut nw = BlockAndWitnessData::new_with_batchsize(
            &ZingoConfig::create_unconnected(ChainType::FakeMainnet, None),
            25_000,
        );

        let cb = FakeCompactBlock::new(1, BlockHash([0u8; 32])).into_cb();
        let blks = vec![BlockData::new(cb)];

        nw.setup_sync(blks.clone(), None).await;
        let finished_blks = nw.drain_existingblocks_into_blocks_with_truncation(1).await;

        assert_eq!(blks[0].hash(), finished_blks[0].hash());
        assert_eq!(blks[0].height, finished_blks[0].height);
    }

    #[tokio::test]
    async fn verify_block_and_witness_data_blocks_order() {
        let mut scenario_bawd = BlockAndWitnessData::new_with_batchsize(
            &ZingoConfig::create_unconnected(ChainType::FakeMainnet, None),
            25_000,
        );

        for numblocks in [0, 1, 2, 10] {
            let existing_blocks = FakeCompactBlockList::new(numblocks).into_blockdatas();
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
        let mut nw = BlockAndWitnessData::new_with_batchsize(
            &ZingoConfig::create_unconnected(ChainType::FakeMainnet, None),
            25_000,
        );

        let existing_blocks = FakeCompactBlockList::new(200).into_blockdatas();
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
        let config = ZingoConfig::create_unconnected(ChainType::FakeMainnet, None);

        let blocks = FakeCompactBlockList::new(200).into_blockdatas();

        // Blocks are in reverse order
        assert!(blocks.first().unwrap().height > blocks.last().unwrap().height);

        let start_block = blocks.first().unwrap().height;
        let end_block = blocks.last().unwrap().height;

        let sync_status = Arc::new(RwLock::new(BatchSyncStatus::default()));
        let mut nw = BlockAndWitnessData::new(&config, sync_status);
        nw.setup_sync(vec![], None).await;

        let (reorg_transmitter, mut reorg_receiver) = unbounded_channel();

        let (h, cb_sender) = nw
            .start(
                start_block,
                end_block,
                Arc::new(RwLock::new(TransactionMetadataSet::new())),
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
                return Err(format!("Should not have requested a reorg!"));
            }
            Ok(())
        });

        assert_eq!(h.await.unwrap().unwrap(), end_block);

        join_all(vec![send_h])
            .await
            .into_iter()
            .collect::<Result<Result<(), String>, _>>()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn with_existing_batched() {
        let config = ZingoConfig::create_unconnected(ChainType::FakeMainnet, None);

        let mut blocks = FakeCompactBlockList::new(200).into_blockdatas();

        // Blocks are in reverse order
        assert!(blocks.first().unwrap().height > blocks.last().unwrap().height);

        // Use the first 50 blocks as "existing", and then sync the other 150 blocks.
        let existing_blocks = blocks.split_off(150);

        let start_block = blocks.first().unwrap().height;
        let end_block = blocks.last().unwrap().height;

        let mut nw = BlockAndWitnessData::new_with_batchsize(&config, 25);
        nw.setup_sync(existing_blocks, None).await;

        let (reorg_transmitter, mut reorg_receiver) = unbounded_channel();

        let (h, cb_sender) = nw
            .start(
                start_block,
                end_block,
                Arc::new(RwLock::new(TransactionMetadataSet::new())),
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
                return Err(format!("Should not have requested a reorg!"));
            }
            Ok(())
        });

        assert_eq!(h.await.unwrap().unwrap(), end_block);

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
        let config = ZingoConfig::create_unconnected(ChainType::FakeMainnet, None);

        let mut blocks = FakeCompactBlockList::new(100).into_blockdatas();

        // Blocks are in reverse order
        assert!(blocks.first().unwrap().height > blocks.last().unwrap().height);

        // Use the first 50 blocks as "existing", and then sync the other 50 blocks.
        let existing_blocks = blocks.split_off(50);

        // The first 5 blocks, blocks 46-50 will be reorg'd, so duplicate them
        let num_reorged = 5;
        let mut reorged_blocks = existing_blocks
            .iter()
            .take(num_reorged)
            .map(|b| b.clone())
            .collect::<Vec<_>>();

        // Reset the hashes
        for i in 0..num_reorged {
            let mut hash = [0u8; 32];
            OsRng.fill_bytes(&mut hash);

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
        let mut nw = BlockAndWitnessData::new(&config, sync_status);
        nw.setup_sync(existing_blocks, None).await;

        let (reorg_transmitter, mut reorg_receiver) = unbounded_channel();

        let (h, cb_sender) = nw
            .start(
                start_block,
                end_block,
                Arc::new(RwLock::new(TransactionMetadataSet::new())),
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

        assert_eq!(h.await.unwrap().unwrap(), end_block - num_reorged as u64);

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

    const SAPLING_START: &'static str = "01f2e6144dbd45cf3faafd337fe59916fe5659b4932a4a008b535b8912a8f5c0000131ac2795ef458f5223f929680085935ebd5cb84d4849b3813b03aeb80d812241020001bcc10bd2116f34cd46d0dedef75c489d6ef9b6b551c0521e3b2e56b7f641fb01";
    const ORCHARD_START: &'static str = "000000";
    const SAPLING_END: &'static str = "01ea571adc215d4eac2925efafd854c76a8afc69f07d49155febaa46f979c6616d0003000001dc4b3f1381533d99b536252aaba09b08fa1c1ddc0687eb4ede716e6a5fbfb003";
    const ORCHARD_END: &'static str = "011e9dcd34e245194b66dff86b1cabaa87b29e96df711e4fff72da64e88656090001c3bf930865f0c2139ce1548b0bfabd8341943571e8af619043d84d12b51355071f00000000000000000000000000000000000000000000000000000000000000";
    const BLOCK: &'static str = "10071a209bf92fdc55b77cbae5d13d49b4d64374ebe7ac3c5579134100e600bc01208d052220b84a8818a26321dca19ad1007d4bb99fafa07b048a0b471b1a4bbd0199ea3c0828d2fe9299063a4612203bc5bbb7df345e336288a7ffe8fac6f7dcb4ee6e4d3b4e852bcd7d52fef8d66a2a220a20d2f3948ed5e06345446d753d2688cdb1d949e0fed1e0f271e04197d0c63251023ace0308011220d151d78dec5dcf5e0308f7bfbf00d80be27081766d2cb5070c22b4556aadc88122220a20bd5312efa676f7000e5d660f01bfd946fd11102af75e7f969a0d84931e16f3532a220a20601ed4084186c4499a1bb5000490582649da2df8f2ba3e749a07c5c0aaeb9f0e2a220a20ea571adc215d4eac2925efafd854c76a8afc69f07d49155febaa46f979c6616d329c010a2062faac36e47bf420b90379b7c96cc5956daa0deedb4ed392928ba588fa5aac1012201e9dcd34e245194b66dff86b1cabaa87b29e96df711e4fff72da64e8865609001a20f5153d2a8ee2e211a7966b52e6bb3b089f32f8c55a57df1f245e6f1ec3356b982234e26e07bfb0d3c0bae348a6196ba3f069e3a1861fa4da1895ab4ad86ece7934231f6490c6e26b63298d8464560b41b2941add03ac329c010a2028ac227306fb680dc501ba9c8c51312b29b2eb79d61e5645f5486039462a620b1220c3bf930865f0c2139ce1548b0bfabd8341943571e8af619043d84d12b51355071a200803db70afdeb80b0f47a703df68c62e007e162b8b983b6f2f5df8d7b24c18a322342b1f151f67048590976a3a69e1d8a6c7408c3489bd81696cad15a9ac0a92d20f1b17ebf717f52695aeaf123aaab0ac406d6afd4a";

    fn decode_block() -> CompactBlock {
        <CompactBlock as prost::Message>::decode(&*hex::decode(BLOCK).unwrap()).unwrap()
    }

    #[test]
    fn orchard_starts_empty() {
        let empty_tree =
            CommitmentTree::<MerkleHashOrchard>::read(&*hex::decode(ORCHARD_START).unwrap())
                .unwrap();
        assert_eq!(empty_tree, CommitmentTree::<MerkleHashOrchard>::empty());
    }

    #[test]
    fn get_sapling_end_from_start_plus_block() {
        let mut start_tree =
            CommitmentTree::<sapling::Node>::read(&*hex::decode(SAPLING_START).unwrap()).unwrap();
        let cb = decode_block();

        for compact_transaction in &cb.vtx {
            super::update_tree_with_compact_transaction::<SaplingDomain<ChainType>>(
                &mut start_tree,
                compact_transaction,
            )
        }
        let end_tree =
            CommitmentTree::<sapling::Node>::read(&*hex::decode(SAPLING_END).unwrap()).unwrap();
        assert_eq!(start_tree, end_tree);
    }

    #[test]
    #[ignore]
    fn get_orchard_end_from_start_plus_block() {
        let mut start_tree =
            CommitmentTree::<MerkleHashOrchard>::read(&*hex::decode(ORCHARD_START).unwrap())
                .unwrap();
        let cb = decode_block();

        for compact_transaction in &cb.vtx {
            super::update_tree_with_compact_transaction::<OrchardDomain>(
                &mut start_tree,
                compact_transaction,
            )
        }
        let end_tree =
            CommitmentTree::<MerkleHashOrchard>::read(&*hex::decode(ORCHARD_END).unwrap()).unwrap();
        assert_eq!(start_tree, end_tree);
    }
}

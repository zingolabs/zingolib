#![allow(dead_code)]
#![allow(unused_variables)]

use crate::wallet::{
    NullifierMap, OutputId, ScanTarget, ShardTrees, SyncState, WalletBlock, WalletTransaction,
    traits::{
        SyncBlocks, SyncNullifiers, SyncOutPoints, SyncShardTrees, SyncTransactions, SyncWallet,
    },
};
use incrementalmerkletree::{Hashable as _, Level, Marking, Position, Retention};
use orchard::tree::MerkleHashOrchard;
use std::collections::{BTreeMap, HashMap};
use zcash_protocol::{TxId, consensus::BlockHeight};
use zingo_netutils::lightwallet_protocol::SubtreeRoot;

pub(crate) fn subtree_root(completing_block_height: u64) -> SubtreeRoot {
    SubtreeRoot {
        completing_block_height,
        ..Default::default()
    }
}

pub(crate) fn orchard_subtree_root(root_hash: MerkleHashOrchard) -> SubtreeRoot {
    SubtreeRoot {
        root_hash: root_hash.to_bytes().to_vec(),
        ..Default::default()
    }
}

// An orchard shard tree in the state a wallet reaches once its own scanning has carried the tree past the roots
// the server supplied: shards 0 to 2 are bare server roots, shards 3 and 4 hold the wallet's own leaves, so the
// tree's resume index is 5 while only three roots were ever fetched.
pub(crate) fn orchard_tree_scanned_past_the_fetched_roots() -> ShardTrees {
    let mut shard_trees = ShardTrees::new();
    let tree = &mut shard_trees.orchard;
    // Three server roots land in shards 0 to 2 as bare leaf nodes holding only the hash. Any valid 32 bytes would
    // do, and the empty root keeps the ommers below consistent with the stored shards.
    let fetched_root = MerkleHashOrchard::empty_root(Level::from(crate::witness::SHARD_HEIGHT));
    crate::witness::add_subtree_roots(0, vec![orchard_subtree_root(fetched_root); 3], tree)
        .unwrap();
    // A frontier at the first position of a shard populates that shard with one call, where appending would
    // take a whole shard of leaves to reach the next one. The frontier at shard 3 stands in for the wallet's
    // birthday frontier or scanning landing there, and the one at shard 4 for scanning completing shard 3 and
    // reaching the first commitment of shard 4. A frontier carries one ommer per set bit of its position: shard
    // 3 is binary 11, so it carries the root of shard 2 at the shard level and the root over shards 0 and 1 one
    // level up, and shard 4 is binary 100, so it carries the root over shards 0 to 3 two levels up. The
    // checkpoint heights are arbitrary labels.
    for (shard_index, ommers) in [
        (
            3,
            vec![
                MerkleHashOrchard::empty_root(Level::from(crate::witness::SHARD_HEIGHT)),
                MerkleHashOrchard::empty_root(Level::from(crate::witness::SHARD_HEIGHT + 1)),
            ],
        ),
        (
            4,
            vec![MerkleHashOrchard::empty_root(Level::from(
                crate::witness::SHARD_HEIGHT + 2,
            ))],
        ),
    ] {
        let frontier = incrementalmerkletree::frontier::NonEmptyFrontier::from_parts(
            Position::from(shard_index << crate::witness::SHARD_HEIGHT),
            MerkleHashOrchard::empty_leaf(),
            ommers,
        )
        .unwrap();
        tree.insert_frontier_nodes(
            frontier,
            Retention::Checkpoint {
                id: BlockHeight::from_u32(shard_index as u32 * 100),
                marking: Marking::None,
            },
        )
        .unwrap();
    }
    shard_trees
}

#[derive(Debug, thiserror::Error)]
pub(super) enum MockWalletError {
    #[error("mock error")]
    AnErrorVariant(String),
}

type SyncStatePatch = Box<dyn Fn(&SyncState) -> Result<&SyncState, MockWalletError> + Send>;
type GetBirthdayPatch = Box<dyn Fn(&BlockHeight) -> Result<BlockHeight, MockWalletError> + Send>;
pub(super) struct MockWallet {
    birthday: BlockHeight,
    sync_state: SyncState,
    get_sync_state_patch: Option<SyncStatePatch>,
    get_birthday_patch: Option<GetBirthdayPatch>,
    wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    wallet_transactions: HashMap<TxId, WalletTransaction>,
    nullifier_map: NullifierMap,
    outpoint_map: BTreeMap<OutputId, ScanTarget>,
    shard_trees: ShardTrees,
    transparent_addresses: BTreeMap<crate::keys::transparent::TransparentAddressId, String>,
}
impl MockWalletError {}

pub(super) struct MockWalletBuilder {
    birthday: BlockHeight,
    get_sync_state_patch: Option<SyncStatePatch>,
    get_birthday_patch: Option<GetBirthdayPatch>,
    sync_state: SyncState,
    wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    wallet_transactions: HashMap<TxId, WalletTransaction>,
    nullifier_map: NullifierMap,
    outpoint_map: BTreeMap<OutputId, ScanTarget>,
    shard_trees: ShardTrees,
}

impl MockWalletBuilder {
    pub(crate) fn birthday(mut self, birthday: BlockHeight) -> Self {
        self.birthday = birthday;
        self
    }

    pub(crate) fn get_sync_state_patch(mut self, patch_fn: SyncStatePatch) -> Self {
        self.get_sync_state_patch = Some(patch_fn);
        self
    }
    pub(crate) fn get_birthday_patch(mut self, patch_fn: GetBirthdayPatch) -> Self {
        self.get_birthday_patch = Some(patch_fn);
        self
    }
    pub(crate) fn sync_state(mut self, sync_state: SyncState) -> Self {
        self.sync_state = sync_state;
        self
    }

    pub(crate) fn wallet_blocks(
        mut self,
        wallet_blocks: BTreeMap<BlockHeight, WalletBlock>,
    ) -> Self {
        self.wallet_blocks = wallet_blocks;
        self
    }

    pub(crate) fn wallet_transactions(
        mut self,
        wallet_transactions: HashMap<TxId, WalletTransaction>,
    ) -> Self {
        self.wallet_transactions = wallet_transactions;
        self
    }

    pub(crate) fn nullifier_map(mut self, nullifier_map: NullifierMap) -> Self {
        self.nullifier_map = nullifier_map;
        self
    }

    pub(crate) fn outpoint_map(mut self, outpoint_map: BTreeMap<OutputId, ScanTarget>) -> Self {
        self.outpoint_map = outpoint_map;
        self
    }

    pub(crate) fn shard_trees(mut self, shard_trees: ShardTrees) -> Self {
        self.shard_trees = shard_trees;
        self
    }
    pub(crate) fn new() -> MockWalletBuilder {
        MockWalletBuilder::default()
    }
    pub(crate) fn create_mock_wallet(self) -> MockWallet {
        MockWallet {
            birthday: self.birthday,
            get_sync_state_patch: self.get_sync_state_patch,
            get_birthday_patch: self.get_birthday_patch,
            sync_state: self.sync_state,
            wallet_blocks: self.wallet_blocks,
            wallet_transactions: self.wallet_transactions,
            nullifier_map: self.nullifier_map,
            outpoint_map: self.outpoint_map,
            shard_trees: self.shard_trees,
            transparent_addresses: BTreeMap::new(),
        }
    }
}
impl Default for MockWalletBuilder {
    fn default() -> Self {
        MockWalletBuilder {
            birthday: BlockHeight::from_u32(0),
            sync_state: SyncState::new(),
            get_sync_state_patch: None,
            get_birthday_patch: None,
            wallet_blocks: BTreeMap::new(),
            wallet_transactions: HashMap::new(),
            nullifier_map: NullifierMap::new(),
            outpoint_map: BTreeMap::new(),
            shard_trees: ShardTrees::new(),
        }
    }
}

impl SyncWallet for MockWallet {
    type Error = MockWalletError;

    fn get_birthday(&self) -> Result<BlockHeight, Self::Error> {
        if let Some(patch) = &self.get_birthday_patch {
            return patch(&self.birthday);
        }
        Ok(self.birthday)
    }

    fn get_sync_state(&self) -> Result<&crate::wallet::SyncState, Self::Error> {
        if let Some(patch) = &self.get_sync_state_patch {
            return patch(&self.sync_state);
        }
        Ok(&self.sync_state)
    }

    fn get_sync_state_mut(&mut self) -> Result<&mut crate::wallet::SyncState, Self::Error> {
        Ok(&mut self.sync_state)
    }

    fn get_unified_full_viewing_keys(
        &self,
    ) -> Result<
        std::collections::HashMap<zip32::AccountId, zcash_keys::keys::UnifiedFullViewingKey>,
        Self::Error,
    > {
        todo!()
    }

    /// The mock wallet does not keep shielded addresses, so a discovered address is accepted and dropped.
    fn add_orchard_address(
        &mut self,
        _account_id: zip32::AccountId,
        _address: orchard::Address,
        _diversifier_index: zip32::DiversifierIndex,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    /// The mock wallet does not keep shielded addresses, so a discovered address is accepted and dropped.
    fn add_sapling_address(
        &mut self,
        _account_id: zip32::AccountId,
        _address: sapling_crypto::PaymentAddress,
        _diversifier_index: zip32::DiversifierIndex,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn get_transparent_addresses(
        &self,
    ) -> Result<
        &std::collections::BTreeMap<crate::keys::transparent::TransparentAddressId, String>,
        Self::Error,
    > {
        Ok(&self.transparent_addresses)
    }

    fn get_transparent_addresses_mut(
        &mut self,
    ) -> Result<
        &mut std::collections::BTreeMap<crate::keys::transparent::TransparentAddressId, String>,
        Self::Error,
    > {
        Ok(&mut self.transparent_addresses)
    }

    fn set_save_flag(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
impl SyncBlocks for MockWallet {
    fn get_wallet_block(
        &self,
        block_height: BlockHeight,
    ) -> Result<crate::wallet::WalletBlock, Self::Error> {
        self.wallet_blocks
            .get(&block_height)
            .cloned()
            .ok_or_else(|| {
                MockWalletError::AnErrorVariant(format!("no wallet block at {block_height}"))
            })
    }

    fn get_wallet_blocks_mut(
        &mut self,
    ) -> Result<&mut std::collections::BTreeMap<BlockHeight, crate::wallet::WalletBlock>, Self::Error>
    {
        Ok(&mut self.wallet_blocks)
    }
}
impl SyncTransactions for MockWallet {
    fn get_wallet_transactions(
        &self,
    ) -> Result<
        &std::collections::HashMap<zcash_protocol::TxId, crate::wallet::WalletTransaction>,
        Self::Error,
    > {
        Ok(&self.wallet_transactions)
    }

    fn get_wallet_transactions_mut(
        &mut self,
    ) -> Result<
        &mut std::collections::HashMap<zcash_protocol::TxId, crate::wallet::WalletTransaction>,
        Self::Error,
    > {
        Ok(&mut self.wallet_transactions)
    }
}
impl SyncNullifiers for MockWallet {
    fn get_nullifiers(&self) -> Result<&crate::wallet::NullifierMap, Self::Error> {
        Ok(&self.nullifier_map)
    }

    fn get_nullifiers_mut(&mut self) -> Result<&mut crate::wallet::NullifierMap, Self::Error> {
        Ok(&mut self.nullifier_map)
    }
}
impl SyncOutPoints for MockWallet {
    fn get_outpoints(
        &self,
    ) -> Result<
        &std::collections::BTreeMap<crate::wallet::OutputId, crate::wallet::ScanTarget>,
        Self::Error,
    > {
        Ok(&self.outpoint_map)
    }

    fn get_outpoints_mut(
        &mut self,
    ) -> Result<
        &mut std::collections::BTreeMap<crate::wallet::OutputId, crate::wallet::ScanTarget>,
        Self::Error,
    > {
        Ok(&mut self.outpoint_map)
    }
}
impl SyncShardTrees for MockWallet {
    fn get_shard_trees(&self) -> Result<&crate::wallet::ShardTrees, Self::Error> {
        todo!()
    }

    fn get_shard_trees_mut(&mut self) -> Result<&mut crate::wallet::ShardTrees, Self::Error> {
        Ok(&mut self.shard_trees)
    }
}

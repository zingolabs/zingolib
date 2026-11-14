use std::{collections::BTreeSet, ops::Range};

use incrementalmerkletree::Position;
use shardtree::{ShardTree, store::memory::MemoryShardStore};
use zcash_address::unified::ParseError;
use zcash_keys::address::UnifiedAddress;
use zcash_primitives::{block::BlockHash, transaction::TxId};
use zcash_protocol::{
    ShieldedPool,
    consensus::{self, BlockHeight},
    memo::Memo,
    value::Zatoshis,
};
use zcash_transparent::address::Script;
use zip32::Scope;

use crate::{
    keys::{KeyId, transparent::TransparentAddressId},
    shardtree_ext::{CheckpointAppendOutcome, ShardTreeExt as _},
    status::ConfirmationStatus,
    sync::{InitialSyncState, ScanPriority, ScanRange, SyncState},
    wallet::{
        IronwoodNote, OrchardNote, OutgoingIronwoodNote, OutgoingNote, OutgoingOrchardNote,
        OutgoingSaplingNote, OutputId, SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE, SaplingNote,
        ScanTarget, ShardTrees, TransparentCoin, TreeBounds, WalletBlock, WalletNote,
        WalletTransaction,
    },
};

pub use crate::witness::SHARD_HEIGHT;

// Where a scan target came from, which decides what a transaction the server does not serve means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanTargetOrigin {
    // Trial decryption, spend detection or transparent address discovery. A transaction the server does not serve
    // is an inconsistency in the served data.
    Internal,
    // `crate::add_scan_targets`. A transaction the server does not serve, or a block height outside the chain, is a
    // consumer error and the target is dropped.
    External,
}

/// An empty shard tree with the crate's standard checkpoint retention and
/// the zero-height checkpoint that every fresh tree must carry.
pub fn empty_shard_tree<H, const DEPTH: u8, const SHARD_HEIGHT: u8>()
-> ShardTree<MemoryShardStore<H, BlockHeight>, DEPTH, SHARD_HEIGHT>
where
    H: incrementalmerkletree::Hashable + Clone + PartialEq,
{
    let mut tree = ShardTree::new(
        MemoryShardStore::empty(),
        SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE as usize,
    );

    // `NotAboveNewest` is impossible on an empty checkpoint store.
    assert_eq!(
        tree.append_checkpoint(BlockHeight::from_u32(0))
            .expect("should never fail"),
        CheckpointAppendOutcome::Appended
    );

    tree
}

pub fn encode_orchard_receiver(
    parameters: &impl consensus::Parameters,
    orchard_address: &orchard::Address,
) -> Result<String, ParseError> {
    Ok(zcash_address::unified::Encoding::encode(
        &<zcash_address::unified::Address as zcash_address::unified::Encoding>::try_from_items(
            zcash_address::unified::Revision::R0,
            vec![zcash_address::unified::Uitem::Data(
                zcash_address::unified::Receiver::Orchard(orchard_address.to_raw_address_bytes()),
            )],
        )?,
        &parameters.network_type(),
    ))
}

pub const VERIFY_BLOCK_RANGE_SIZE: u32 = 10;

/// Returns true when the two ranges share at least one block.
pub fn overlaps(first: &Range<BlockHeight>, second: &Range<BlockHeight>) -> bool {
    first.start < second.end && second.start < first.end
}

/// Used to determine which end of the scan range is verified.
pub enum VerifyEnd {
    VerifyHighest,
    VerifyLowest,
}

mod sealed {
    pub trait Sealed {}
}

impl sealed::Sealed for SyncState {}
impl sealed::Sealed for InitialSyncState {}
impl sealed::Sealed for ScanTarget {}
impl sealed::Sealed for WalletBlock {}
impl sealed::Sealed for WalletTransaction {}
impl sealed::Sealed for TransparentCoin {}
impl<N, Nf: Copy, P> sealed::Sealed for WalletNote<N, Nf, P> {}
impl<N, P> sealed::Sealed for OutgoingNote<N, P> {}
impl sealed::Sealed for KeyId {}
impl sealed::Sealed for ShardTrees {}

pub trait ScanTargetOps: sealed::Sealed {
    fn internal(block_height: BlockHeight, txid: TxId) -> Self;
    fn origin(&self) -> ScanTargetOrigin;
}

pub trait WalletBlockOps: sealed::Sealed {
    #[must_use]
    fn from_parts(
        block_height: BlockHeight,
        block_hash: BlockHash,
        prev_hash: BlockHash,
        time: u32,
        txids: Vec<TxId>,
        tree_bounds: TreeBounds,
    ) -> Self;
}

pub trait WalletTransactionOps: sealed::Sealed {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    fn from_parts(
        txid: TxId,
        status: ConfirmationStatus,
        transaction: zcash_primitives::transaction::Transaction,
        datetime: u32,
        transparent_coins: Vec<TransparentCoin>,
        sapling_notes: Vec<SaplingNote>,
        orchard_notes: Vec<OrchardNote>,
        ironwood_notes: Vec<IronwoodNote>,
        outgoing_sapling_notes: Vec<OutgoingSaplingNote>,
        outgoing_orchard_notes: Vec<OutgoingOrchardNote>,
        outgoing_ironwood_notes: Vec<OutgoingIronwoodNote>,
    ) -> Self;
    fn set_refetch_nullifier_ranges(&mut self, refetch_nullifier_ranges: &[Range<BlockHeight>]);
}

pub trait TransparentCoinOps: sealed::Sealed {
    #[must_use]
    fn from_parts(
        output_id: OutputId,
        key_id: TransparentAddressId,
        address: String,
        script: Script,
        value: Zatoshis,
    ) -> Self;
}

pub trait WalletNoteOps<N, Nf>: sealed::Sealed {
    #[must_use]
    fn from_parts(
        output_id: OutputId,
        key_id: KeyId,
        note: N,
        nullifier: Option<Nf>,
        position: Option<Position>,
        memo: Memo,
    ) -> Self;
}

pub trait OutgoingNoteOps<N>: sealed::Sealed {
    #[must_use]
    fn from_parts(output_id: OutputId, key_id: KeyId, note: N, memo: Memo) -> Self;
    fn set_recipient_full_unified_address(
        &mut self,
        recipient_full_unified_address: Option<UnifiedAddress>,
    );
}

pub trait KeyIdOps: sealed::Sealed {
    fn from_parts(account_id: zip32::AccountId, scope: Scope) -> Self;
}

pub trait ShardTreesOps: sealed::Sealed {
    fn clear_pool(&mut self, pool: ShieldedPool);
}

pub trait SyncStateOps: sealed::Sealed {
    fn scan_complete(&self) -> bool;
    fn insert_scan_target(&mut self, scan_target: ScanTarget);
    #[must_use]
    fn scan_targets(&self) -> &BTreeSet<ScanTarget>;
    #[must_use]
    fn initial_sync_state(&self) -> &InitialSyncState;
    #[must_use]
    fn transparent_scan_floor(&self) -> Option<BlockHeight>;
    fn set_initial_sync_state(&mut self, initial_sync_state: InitialSyncState);
    fn set_transparent_scan_floor(&mut self, height: BlockHeight);
    fn retain_scan_targets(&mut self, keep: impl FnMut(&ScanTarget) -> bool);
    fn set_wallet_tree_bounds(&mut self, wallet_tree_bounds: TreeBounds);
    fn find_scan_targets(&self, block_range: &Range<BlockHeight>) -> BTreeSet<ScanTarget>;
    fn drop_external_scan_targets_outside_the_chain(&mut self, chain_height: BlockHeight);
    fn drop_settled_scan_targets(&mut self, fully_scanned_height: BlockHeight);
    fn merge_scan_ranges(&mut self, scan_priority: ScanPriority);
    fn create_scan_range(
        &mut self,
        last_known_chain_height: BlockHeight,
        chain_height: BlockHeight,
    );
    fn truncate_scan_ranges(&mut self, truncate_height: BlockHeight);
    fn reset_scan_ranges(&mut self);
    fn set_verify_scan_range(
        &mut self,
        block_height: BlockHeight,
        verify_end: VerifyEnd,
    ) -> ScanRange;
    fn set_chain_tip_scan_range(
        &mut self,
        consensus_parameters: &impl consensus::Parameters,
        chain_height: BlockHeight,
    );
    fn set_found_note_scan_ranges<T: Iterator<Item = ScanTarget>>(
        &mut self,
        consensus_parameters: &impl consensus::Parameters,
        scan_targets: T,
    );
    fn set_scanned_scan_range(
        &mut self,
        scanned_range: Range<BlockHeight>,
        nullifiers_mapped: bool,
    );
    fn reset_refetching_nullifiers_scan_range(&mut self, invalid_refetch_range: Range<BlockHeight>);
    fn reset_stale_load(&mut self, task_priority: ScanPriority, reset: &[Range<BlockHeight>]);
    fn reset_verify_scan_range(&mut self, block_range: &Range<BlockHeight>);
    fn set_scan_priority(&mut self, block_range: &Range<BlockHeight>, scan_priority: ScanPriority);
    fn select_scan_range(
        &mut self,
        consensus_parameters: &impl consensus::Parameters,
        nullifier_map_limit_exceeded: bool,
    ) -> Option<ScanRange>;
    fn lower_transparent_scan_floor(&mut self, height: BlockHeight);
    fn calculate_scanned_blocks(&self) -> u32;
    fn clear_shard_ranges(&mut self, shielded_protocol: ShieldedPool);
    fn reopen_scan_ranges_inner(&mut self, from_height: BlockHeight);
    fn add_shard_ranges(
        &mut self,
        consensus_parameters: &impl consensus::Parameters,
        shielded_protocol: ShieldedPool,
        start_index: u32,
        subtree_completing_heights: &[BlockHeight],
    );
    fn update_found_note_shard_priority(
        &mut self,
        consensus_parameters: &impl consensus::Parameters,
        shielded_protocol: ShieldedPool,
        wallet_transaction: &WalletTransaction,
    );
}

pub trait InitialSyncStateOps: sealed::Sealed {
    #[must_use]
    fn from_parts(
        sync_start_height: BlockHeight,
        wallet_tree_bounds: TreeBounds,
        previously_scanned_blocks: u32,
        previously_scanned_sapling_outputs: u32,
        previously_scanned_orchard_outputs: u32,
        previously_scanned_ironwood_outputs: u32,
    ) -> Self;
    #[must_use]
    fn sync_start_height(&self) -> BlockHeight;
    #[must_use]
    fn wallet_tree_bounds(&self) -> TreeBounds;
    #[must_use]
    fn previously_scanned_blocks(&self) -> u32;
    #[must_use]
    fn previously_scanned_sapling_outputs(&self) -> u32;
    #[must_use]
    fn previously_scanned_orchard_outputs(&self) -> u32;
    #[must_use]
    fn previously_scanned_ironwood_outputs(&self) -> u32;
}

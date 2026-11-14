use std::{collections::BTreeSet, ops::Range};

use zcash_protocol::{ShieldedPool, consensus::BlockHeight};

use crate::{
    engine::{InitialSyncStateOps, SyncStateOps, overlaps},
    wallet::{ScanTarget, TreeBounds},
};

pub use crate::wallet::{MAX_REORG_ALLOWANCE, SHARDTREE_CHECKPOINT_ROLLING_WINDOW_SIZE};

mod state;

/// API for targetted scanning.
///
/// Adds `scan_targets` to the wallet's `sync_state`, where they are prioritised for scanning at the start of every
/// sync session until the fully scanned height passes them. The narrow range of blocks around each target's block
/// height is prioritised, and once a transaction at a target is found to hold the wallet's notes, the whole shard of
/// each such pool is prioritised instead, so the notes become spendable. A target's txid is scanned there when given,
/// and a target is dropped where the server does not serve its transaction or where its block height lies outside
/// the chain range the wallet scans. A spending transaction that holds no decryptable incoming note is scanned when
/// the transaction holding the spent notes is scanned instead, through its nullifier.
pub fn add_scan_targets(sync_state: &mut SyncState, scan_targets: &[ScanTarget]) {
    for scan_target in scan_targets {
        sync_state.insert_scan_target(*scan_target);
    }
}

/// Scanning range priority levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScanPriority {
    /// Block ranges that are currently refetching nullifiers.
    RefetchingNullifiers,
    /// Block ranges that are currently being scanned.
    Scanning,
    /// Block ranges that have already been scanned will not be re-scanned.
    Scanned,
    /// Block ranges that have already been scanned. The nullifiers from this range were not mapped after scanning and
    /// spend detection to reduce memory consumption and/or storage for non-linear scanning. These nullifiers will need
    /// to be re-fetched for final spend detection when this range is the lowest unscanned range in the wallet's list
    /// of scan ranges.
    ScannedWithoutMapping,
    /// Block ranges to be scanned to advance the fully-scanned height.
    Historic,
    /// Block ranges adjacent to heights at which the user opened the wallet.
    OpenAdjacent,
    /// Blocks that must be scanned to complete note commitment tree shards adjacent to found notes.
    FoundNote,
    /// Blocks that must be scanned to complete the latest note commitment tree shard.
    ChainTip,
    /// A previously scanned range that must be verified to check it is still in the
    /// main chain, has highest priority.
    Verify,
}

impl ScanPriority {
    /// Whether this priority marks a range whose blocks have been scanned,
    /// including ranges whose nullifiers still await retrieval. Contrast with
    /// equality to [`ScanPriority::Scanned`], which additionally requires the
    /// nullifier work to be finished.
    pub fn is_scanned(self) -> bool {
        matches!(
            self,
            ScanPriority::Scanned
                | ScanPriority::ScannedWithoutMapping
                | ScanPriority::RefetchingNullifiers
        )
    }

    /// Whether this priority marks a range whose blocks have been scanned but
    /// whose nullifiers still await mapping or refetching for final spend
    /// detection.
    pub fn awaits_nullifier_retrieval(self) -> bool {
        matches!(
            self,
            ScanPriority::ScannedWithoutMapping | ScanPriority::RefetchingNullifiers
        )
    }

    /// Returns the priority the wallet holds a range selected at this priority while its scan task is in flight.
    pub fn in_flight(self) -> ScanPriority {
        if self == ScanPriority::ScannedWithoutMapping {
            ScanPriority::RefetchingNullifiers
        } else {
            ScanPriority::Scanning
        }
    }
}

/// A range of blocks to be scanned, along with its associated priority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRange {
    block_range: Range<BlockHeight>,
    priority: ScanPriority,
}

impl std::fmt::Display for ScanRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?}({}..{})",
            self.priority, self.block_range.start, self.block_range.end,
        )
    }
}

impl ScanRange {
    /// Constructs a scan range from its constituent parts.
    #[must_use]
    pub fn from_parts(block_range: Range<BlockHeight>, priority: ScanPriority) -> Self {
        assert!(
            block_range.end >= block_range.start,
            "{block_range:?} is invalid for ScanRange({priority:?})",
        );
        ScanRange {
            block_range,
            priority,
        }
    }

    /// Returns the range of block heights to be scanned.
    #[must_use]
    pub fn block_range(&self) -> &Range<BlockHeight> {
        &self.block_range
    }

    /// Returns the priority with which the scan range should be scanned.
    #[must_use]
    pub fn priority(&self) -> ScanPriority {
        self.priority
    }

    /// Returns whether or not the scan range is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.block_range.is_empty()
    }

    /// Returns true when `block_range` lies within this scan range.
    #[must_use]
    pub fn encloses(&self, block_range: &Range<BlockHeight>) -> bool {
        self.block_range.start <= block_range.start && block_range.end <= self.block_range.end
    }

    /// Returns true when this scan range and `block_range` share at least one block.
    #[must_use]
    pub fn overlaps(&self, block_range: &Range<BlockHeight>) -> bool {
        overlaps(&self.block_range, block_range)
    }

    /// Returns the number of blocks in the scan range.
    #[must_use]
    pub fn len(&self) -> usize {
        usize::try_from(u32::from(self.block_range.end) - u32::from(self.block_range.start))
            .expect("due to number of max blocks should always be valid usize")
    }

    /// Shifts the start of the block range to the right if `block_height >
    /// self.block_range().start`. Returns `None` if the resulting range would
    /// be empty (or the range was already empty).
    #[must_use]
    pub fn truncate_start(&self, block_height: BlockHeight) -> Option<Self> {
        if block_height >= self.block_range.end || self.is_empty() {
            None
        } else {
            Some(ScanRange {
                block_range: self.block_range.start.max(block_height)..self.block_range.end,
                priority: self.priority,
            })
        }
    }

    /// Shifts the end of the block range to the left if `block_height <
    /// self.block_range().end`. Returns `None` if the resulting range would
    /// be empty (or the range was already empty).
    #[must_use]
    pub fn truncate_end(&self, block_height: BlockHeight) -> Option<Self> {
        if block_height <= self.block_range.start || self.is_empty() {
            None
        } else {
            Some(ScanRange {
                block_range: self.block_range.start..self.block_range.end.min(block_height),
                priority: self.priority,
            })
        }
    }

    /// Splits this scan range at the specified height, such that the provided height becomes the
    /// end of the first range returned and the start of the second. Returns `None` if
    /// `p <= self.block_range().start || p >= self.block_range().end`.
    #[must_use]
    pub fn split_at(&self, p: BlockHeight) -> Option<(Self, Self)> {
        (p > self.block_range.start && p < self.block_range.end).then_some((
            ScanRange {
                block_range: self.block_range.start..p,
                priority: self.priority,
            },
            ScanRange {
                block_range: p..self.block_range.end,
                priority: self.priority,
            },
        ))
    }
}

/// Initial sync state.
///
/// All fields will be reset when a new sync session starts.
#[derive(Debug, Clone)]
pub struct InitialSyncState {
    /// One block above the fully scanned wallet height at start of sync session.
    ///
    /// If chain height is not larger than fully scanned height when sync is called, this value will be set to chain
    /// height instead.
    pub(crate) sync_start_height: BlockHeight,
    /// The tree sizes of the fully scanned height and chain tip at start of sync session.
    pub(crate) wallet_tree_bounds: TreeBounds,
    /// Total number of blocks scanned in previous sync sessions.
    pub(crate) previously_scanned_blocks: u32,
    /// Total number of sapling outputs scanned in previous sync sessions.
    pub(crate) previously_scanned_sapling_outputs: u32,
    /// Total number of orchard outputs scanned in previous sync sessions.
    pub(crate) previously_scanned_orchard_outputs: u32,
    /// Total number of ironwood outputs scanned in previous sync sessions.
    pub(crate) previously_scanned_ironwood_outputs: u32,
}

impl InitialSyncState {
    /// Create new `InitialSyncState`
    #[must_use]
    pub fn new() -> Self {
        InitialSyncState {
            sync_start_height: 0.into(),
            wallet_tree_bounds: TreeBounds {
                sapling_initial_tree_size: 0,
                sapling_final_tree_size: 0,
                orchard_initial_tree_size: 0,
                orchard_final_tree_size: 0,
                ironwood_initial_tree_size: 0,
                ironwood_final_tree_size: 0,
            },
            previously_scanned_blocks: 0,
            previously_scanned_sapling_outputs: 0,
            previously_scanned_orchard_outputs: 0,
            previously_scanned_ironwood_outputs: 0,
        }
    }
}

impl InitialSyncStateOps for InitialSyncState {
    fn from_parts(
        sync_start_height: BlockHeight,
        wallet_tree_bounds: TreeBounds,
        previously_scanned_blocks: u32,
        previously_scanned_sapling_outputs: u32,
        previously_scanned_orchard_outputs: u32,
        previously_scanned_ironwood_outputs: u32,
    ) -> Self {
        Self {
            sync_start_height,
            wallet_tree_bounds,
            previously_scanned_blocks,
            previously_scanned_sapling_outputs,
            previously_scanned_orchard_outputs,
            previously_scanned_ironwood_outputs,
        }
    }

    fn sync_start_height(&self) -> BlockHeight {
        self.sync_start_height
    }

    fn wallet_tree_bounds(&self) -> TreeBounds {
        self.wallet_tree_bounds
    }

    fn previously_scanned_blocks(&self) -> u32 {
        self.previously_scanned_blocks
    }

    fn previously_scanned_sapling_outputs(&self) -> u32 {
        self.previously_scanned_sapling_outputs
    }

    fn previously_scanned_orchard_outputs(&self) -> u32 {
        self.previously_scanned_orchard_outputs
    }

    fn previously_scanned_ironwood_outputs(&self) -> u32 {
        self.previously_scanned_ironwood_outputs
    }
}

impl Default for InitialSyncState {
    fn default() -> Self {
        Self::new()
    }
}

/// Encapsulates the current state of sync
#[derive(Debug, Clone)]
pub struct SyncState {
    /// A vec of block ranges with scan priorities from wallet birthday to chain tip.
    /// In block height order with no overlaps or gaps.
    pub(crate) scan_ranges: Vec<ScanRange>,
    /// The block ranges that contain all sapling outputs of complete sapling shards.
    ///
    /// There is an edge case where a range may include two (or more) shards. However, this only occurs when the lower
    /// shards are already scanned so will cause no issues when punching in the higher scan priorites.
    pub(crate) sapling_shard_ranges: Vec<Range<BlockHeight>>,
    /// The block ranges that contain all orchard outputs of complete orchard shards.
    ///
    /// There is an edge case where a range may include two (or more) shards. However, this only occurs when the lower
    /// shards are already scanned so will cause no issues when punching in the higher scan priorites.
    pub(crate) orchard_shard_ranges: Vec<Range<BlockHeight>>,
    /// The block ranges that contain all ironwood outputs of complete ironwood shards.
    ///
    /// There is an edge case where a range may include two (or more) shards. However, this only occurs when the lower
    /// shards are already scanned so will cause no issues when punching in the higher scan priorites.
    pub(crate) ironwood_shard_ranges: Vec<Range<BlockHeight>>,
    /// Scan targets for relevant transactions to the wallet.
    pub(crate) scan_targets: BTreeSet<ScanTarget>,
    /// Initial sync state.
    pub(crate) initial_sync_state: InitialSyncState,
    /// Compact block transparent data is only scanned above this height. This includes checking transparent outputs
    /// against the wallet's in-use and gap addresses and mapping transparent inputs to the wallet's outpoint map.
    ///
    /// Set to the chain height when transparent address discovery completes at the start of a sync session, as it has
    /// located all relevant transactions at or below this height. Lowered if the wallet is truncated below this
    /// height, by a re-org or a chain height below the wallet's, as transparent address discovery does not cover the
    /// blocks scanned in place of the truncated blocks.
    ///
    /// Blocks mined during a sync session are above the floor and may still be unscanned when the session ends, so
    /// the transparent address discovery of the next sync session searches from the block above the floor.
    pub(crate) transparent_scan_floor: Option<BlockHeight>,
}

impl SyncState {
    /// Create new `SyncState`
    #[must_use]
    pub fn new() -> Self {
        SyncState {
            scan_ranges: Vec::new(),
            sapling_shard_ranges: Vec::new(),
            orchard_shard_ranges: Vec::new(),
            ironwood_shard_ranges: Vec::new(),
            scan_targets: BTreeSet::new(),
            initial_sync_state: InitialSyncState::new(),
            transparent_scan_floor: None,
        }
    }

    /// Scan ranges
    #[must_use]
    pub fn scan_ranges(&self) -> &[ScanRange] {
        &self.scan_ranges
    }

    /// Sapling shard ranges
    #[must_use]
    pub fn sapling_shard_ranges(&self) -> &[Range<BlockHeight>] {
        &self.sapling_shard_ranges
    }

    /// Orchard shard ranges
    #[must_use]
    pub fn orchard_shard_ranges(&self) -> &[Range<BlockHeight>] {
        &self.orchard_shard_ranges
    }

    /// Ironwood shard ranges
    #[must_use]
    pub fn ironwood_shard_ranges(&self) -> &[Range<BlockHeight>] {
        &self.ironwood_shard_ranges
    }

    /// The shard ranges of `pool`
    #[must_use]
    pub fn shard_ranges(&self, pool: ShieldedPool) -> &[Range<BlockHeight>] {
        match pool {
            ShieldedPool::Sapling => &self.sapling_shard_ranges,
            ShieldedPool::Orchard => &self.orchard_shard_ranges,
            ShieldedPool::Ironwood => &self.ironwood_shard_ranges,
        }
    }

    /// Returns the block height at which all blocks equal to and below this height are scanned.
    /// Returns `None` if `self.scan_ranges` is empty.
    #[must_use]
    pub fn fully_scanned_height(&self) -> Option<BlockHeight> {
        if let Some(scan_range) = self
            .scan_ranges
            .iter()
            .find(|scan_range| scan_range.priority() != ScanPriority::Scanned)
        {
            Some(scan_range.block_range().start - 1)
        } else {
            self.scan_ranges
                .last()
                .map(|range| range.block_range().end - 1)
        }
    }

    /// Returns the highest block height that has been scanned.
    /// If no scan ranges have been scanned, returns the block below the wallet birthday.
    /// Returns `None` if `self.scan_ranges` is empty.
    #[must_use]
    pub fn highest_scanned_height(&self) -> Option<BlockHeight> {
        if let Some(last_scanned_range) = self.scan_ranges.iter().rfind(|scan_range| {
            scan_range.priority() == ScanPriority::Scanned
                || scan_range.priority() == ScanPriority::ScannedWithoutMapping
                || scan_range.priority() == ScanPriority::RefetchingNullifiers
        }) {
            Some(last_scanned_range.block_range().end - 1)
        } else {
            self.wallet_birthday().map(|start| start - 1)
        }
    }

    /// Returns the wallet birthday or `None` if `self.scan_ranges` is empty.
    ///
    #[must_use]
    pub fn wallet_birthday(&self) -> Option<BlockHeight> {
        self.scan_ranges
            .first()
            .map(|range| range.block_range().start)
    }

    /// Returns the last known chain height to the wallet or `None` if `self.scan_ranges` is empty.
    #[must_use]
    pub fn last_known_chain_height(&self) -> Option<BlockHeight> {
        self.scan_ranges
            .last()
            .map(|range| range.block_range().end - 1)
    }
}

impl Default for SyncState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "test-features"))]
impl SyncState {
    /// Creates sync state with the given scan ranges, for tests exercising
    /// spendability/witness gating without a chain.
    pub fn new_for_test(scan_ranges: Vec<ScanRange>) -> Self {
        let mut sync_state = Self::new();
        sync_state.scan_ranges = scan_ranges;
        sync_state
    }

    /// - Replaces the shard ranges of `pool` with `shard_ranges`.
    pub fn set_shard_ranges_for_test(
        &mut self,
        pool: ShieldedPool,
        shard_ranges: Vec<Range<BlockHeight>>,
    ) {
        match pool {
            ShieldedPool::Sapling => self.sapling_shard_ranges = shard_ranges,
            ShieldedPool::Orchard => self.orchard_shard_ranges = shard_ranges,
            ShieldedPool::Ironwood => self.ironwood_shard_ranges = shard_ranges,
        }
    }
}

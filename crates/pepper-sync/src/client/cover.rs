//! Cover for transaction requests.
//!
//! The chain is divided into aligned areas of [`AREA_BLOCKS`] blocks. Within an area, blocks are grouped from the
//! first block of the area, and a group closes at the block that brings it to [`GROUP_OUTPUTS`] shielded outputs.
//! To fetch one transaction, sync requests every transaction of its group. In the open group at the chain tip,
//! sync requests every transaction of each block it scans.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    future::Future,
    ops::Range,
};

use zcash_primitives::transaction::TxId;
use zcash_protocol::{ShieldedPool, consensus::BlockHeight};
use zl_netutils::lightwallet_protocol::{CompactBlock, RawTransaction};

use crate::utils::{block, transaction::get_compact_txid};

pub(crate) const AREA_BLOCKS: u32 = 1_000;
pub(crate) const GROUP_OUTPUTS: u32 = 128;
const MAX_CACHED_AREAS: usize = 4;
const MAX_CACHED_TRANSACTIONS: usize = 2_048;

#[derive(Debug)]
pub struct CoverRequest {
    pub targets: Vec<(BlockHeight, TxId)>,
    pub scanned_heights: Range<BlockHeight>,
    pub timed_heights: Vec<BlockHeight>,
    pub lowest_height: Option<BlockHeight>,
    pub chain_height: BlockHeight,
}

impl CoverRequest {
    /// Returns `true` if the request needs no data from the server.
    pub(crate) fn is_empty(&self) -> bool {
        self.targets.is_empty() && self.timed_heights.is_empty() && self.scanned_heights.is_empty()
    }
}

#[derive(Debug, Default)]
pub struct CoverFetch {
    pub target_replies: HashMap<TxId, Result<RawTransaction, tonic::Status>>,
    pub block_times: HashMap<BlockHeight, u32>,
}

/// The server requests that a cover fetch makes.
pub(crate) trait CoverSource {
    /// Fetches the compact blocks of `block_range`, with the data of every pool.
    fn compact_blocks(
        &mut self,
        block_range: Range<BlockHeight>,
    ) -> impl Future<Output = Result<Vec<CompactBlock>, tonic::Status>> + Send;

    /// Fetches the transaction `txid`.
    fn transaction(
        &mut self,
        txid: TxId,
    ) -> impl Future<Output = Result<RawTransaction, tonic::Status>> + Send;
}

struct AreaBlock {
    height: BlockHeight,
    hash: Vec<u8>,
    time: u32,
    outputs: u32,
    txids: Vec<TxId>,
}

impl From<CompactBlock> for AreaBlock {
    fn from(compact_block: CompactBlock) -> Self {
        Self {
            height: block::get_compact_height(&compact_block),
            outputs: [
                ShieldedPool::Sapling,
                ShieldedPool::Orchard,
                ShieldedPool::Ironwood,
            ]
            .into_iter()
            .map(|pool| block::shielded_output_count(&compact_block, pool))
            .fold(0, u32::saturating_add),
            txids: compact_block.vtx.iter().map(get_compact_txid).collect(),
            time: compact_block.time,
            hash: compact_block.hash,
        }
    }
}

/// The areas and transactions fetched in one sync session.
#[derive(Default)]
pub(crate) struct CoverCache {
    areas: HashMap<BlockHeight, Vec<AreaBlock>>,
    transactions: HashMap<TxId, RawTransaction>,
    transaction_order: VecDeque<TxId>,
}

impl CoverCache {
    fn insert_transaction(&mut self, txid: TxId, raw_transaction: RawTransaction) {
        if self.transactions.insert(txid, raw_transaction).is_none() {
            self.transaction_order.push_back(txid);
        }
    }

    fn evict(&mut self, areas_in_use: &BTreeSet<BlockHeight>) {
        while self.transaction_order.len() > MAX_CACHED_TRANSACTIONS {
            if let Some(txid) = self.transaction_order.pop_front() {
                self.transactions.remove(&txid);
            }
        }
        if self.areas.len() > MAX_CACHED_AREAS {
            self.areas
                .retain(|area_start, _| areas_in_use.contains(area_start));
        }
    }
}

fn area_start(height: BlockHeight) -> BlockHeight {
    height - u32::from(height) % AREA_BLOCKS
}

/// Splits the blocks of an area into closed groups and the open group, as index ranges into `outputs`.
///
/// In a complete area the blocks after the last closed group join that group, and the open group is empty.
fn group_blocks(outputs: &[u32], area_is_complete: bool) -> (Vec<Range<usize>>, Range<usize>) {
    let mut closed = Vec::new();
    let mut group_start = 0;
    let mut group_outputs = 0u32;
    for (index, block_outputs) in outputs.iter().enumerate() {
        group_outputs = group_outputs.saturating_add(*block_outputs);
        if group_outputs >= GROUP_OUTPUTS {
            closed.push(group_start..index + 1);
            group_start = index + 1;
            group_outputs = 0;
        }
    }
    if area_is_complete && group_start < outputs.len() {
        match closed.last_mut() {
            Some(last_group) => last_group.end = outputs.len(),
            None => closed.push(0..outputs.len()),
        }
        group_start = outputs.len();
    }

    (closed, group_start..outputs.len())
}

/// Returns the blocks of the area at `area_start`, from `block_range.start` to `block_range.end`.
///
/// Cached blocks are kept if the newest of them is still on the chain.
async fn load_area(
    source: &mut impl CoverSource,
    cache: &mut CoverCache,
    area_start: BlockHeight,
    block_range: Range<BlockHeight>,
    area_is_complete: bool,
) -> Result<(), tonic::Status> {
    let mut blocks = cache.areas.remove(&area_start).unwrap_or_default();
    blocks.retain(|block| block_range.contains(&block.height));
    let is_whole =
        u32::try_from(blocks.len()).is_ok_and(|count| block_range.start + count == block_range.end);
    if let Some(newest) = blocks.last().filter(|_| !(area_is_complete && is_whole)) {
        let mut fetched = source
            .compact_blocks(newest.height..block_range.end)
            .await?
            .into_iter()
            .map(AreaBlock::from);
        if fetched
            .next()
            .is_some_and(|refetched| refetched.hash == newest.hash)
        {
            blocks.extend(fetched);
        } else {
            blocks.clear();
        }
    }
    if blocks.is_empty() && !block_range.is_empty() {
        blocks = source
            .compact_blocks(block_range)
            .await?
            .into_iter()
            .map(AreaBlock::from)
            .collect();
    }
    cache.areas.insert(area_start, blocks);

    Ok(())
}

/// Fetches the targets of `request` together with every transaction of their groups.
///
/// Also fetches every transaction of each scanned block in the open group at the chain tip.
pub(crate) async fn fetch(
    source: &mut impl CoverSource,
    cache: &mut CoverCache,
    request: CoverRequest,
) -> Result<CoverFetch, tonic::Status> {
    let chain_end = request.chain_height + 1;
    let tip_area_start = area_start(request.chain_height);
    let scanned_tip_heights = request.scanned_heights.start.max(tip_area_start)
        ..request.scanned_heights.end.min(chain_end);

    let mut wanted_heights: BTreeMap<BlockHeight, BTreeSet<BlockHeight>> = BTreeMap::new();
    for height in request
        .targets
        .iter()
        .map(|(height, _)| *height)
        .chain(request.timed_heights.iter().copied())
    {
        wanted_heights
            .entry(area_start(height))
            .or_default()
            .insert(height);
    }
    if !scanned_tip_heights.is_empty() {
        wanted_heights.entry(tip_area_start).or_default();
    }
    let target_heights = request
        .targets
        .iter()
        .map(|(height, _)| *height)
        .collect::<HashSet<_>>();

    let mut units: BTreeMap<BlockHeight, Vec<TxId>> = BTreeMap::new();
    let mut block_times = HashMap::new();
    for (area_start, heights) in &wanted_heights {
        let block_range = request
            .lowest_height
            .map_or(*area_start, |lowest_height| lowest_height.max(*area_start))
            ..(*area_start + AREA_BLOCKS).min(chain_end);
        let area_is_complete = *area_start + AREA_BLOCKS <= chain_end;
        load_area(
            source,
            cache,
            *area_start,
            block_range.clone(),
            area_is_complete,
        )
        .await?;
        let blocks = &cache.areas[area_start];
        let outputs = blocks.iter().map(|block| block.outputs).collect::<Vec<_>>();
        let (closed_groups, open_group) = group_blocks(&outputs, area_is_complete);
        let index_of = |height: BlockHeight| {
            usize::try_from(u32::from(height).checked_sub(u32::from(block_range.start))?)
                .ok()
                .filter(|index| {
                    blocks
                        .get(*index)
                        .is_some_and(|block| block.height == height)
                })
        };
        let mut add_unit = |unit: Range<usize>| {
            units.entry(blocks[unit.start].height).or_insert_with(|| {
                blocks[unit]
                    .iter()
                    .flat_map(|block| block.txids.iter().copied())
                    .collect()
            });
        };

        for height in heights {
            let Some(index) = index_of(*height) else {
                continue;
            };
            block_times.insert(*height, blocks[index].time);
            if !target_heights.contains(height) {
                continue;
            }
            match closed_groups.iter().find(|group| group.contains(&index)) {
                Some(group) => add_unit(group.clone()),
                None => add_unit(index..index + 1),
            }
        }
        if *area_start == tip_area_start {
            for index in (u32::from(scanned_tip_heights.start)..u32::from(scanned_tip_heights.end))
                .filter_map(|height| index_of(BlockHeight::from_u32(height)))
                .filter(|index| open_group.contains(index))
            {
                add_unit(index..index + 1);
            }
        }
    }

    let target_txids = request
        .targets
        .iter()
        .map(|(_, txid)| *txid)
        .collect::<HashSet<_>>();
    let mut target_replies = HashMap::with_capacity(target_txids.len());
    for txids in units.values() {
        let is_cached = txids
            .iter()
            .all(|txid| cache.transactions.contains_key(txid));
        for txid in txids {
            let is_target = target_txids.contains(txid);
            if is_cached {
                if is_target && let Some(raw_transaction) = cache.transactions.get(txid) {
                    target_replies.insert(*txid, Ok(raw_transaction.clone()));
                }
                continue;
            }
            match source.transaction(*txid).await {
                Ok(raw_transaction) => {
                    if is_target {
                        target_replies.insert(*txid, Ok(raw_transaction.clone()));
                    }
                    cache.insert_transaction(*txid, raw_transaction);
                }
                Err(status) if is_target => {
                    target_replies.insert(*txid, Err(status));
                }
                Err(_) => {}
            }
        }
    }
    for (_, txid) in &request.targets {
        if !target_replies.contains_key(txid) {
            target_replies.insert(*txid, source.transaction(*txid).await);
        }
    }
    cache.evict(&wanted_heights.keys().copied().collect());

    Ok(CoverFetch {
        target_replies,
        block_times,
    })
}

#[cfg(test)]
mod tests {
    use zl_netutils::lightwallet_protocol::{CompactOrchardAction, CompactTx};

    use super::*;

    const LOWEST_HEIGHT: Option<BlockHeight> = Some(BlockHeight::from_u32(1));

    fn txid(block_height: u32, index: u8) -> TxId {
        let mut bytes = [index; 32];
        bytes[..4].copy_from_slice(&block_height.to_le_bytes());
        TxId::from_bytes(bytes)
    }

    fn block_txids(block_heights: Range<u32>) -> Vec<TxId> {
        block_heights
            .flat_map(|height| [txid(height, 1), txid(height, 2)])
            .collect()
    }

    /// A chain with two transactions and `outputs_per_block` shielded outputs in every block.
    #[derive(Default)]
    struct TestChain {
        outputs_per_block: usize,
        unserved: Vec<TxId>,
        replaced_blocks: Vec<u32>,
        block_failure: Option<tonic::Code>,
        served_blocks_end: Option<u32>,
        served_height: u64,
        block_requests: Vec<Range<u32>>,
        transaction_requests: Vec<TxId>,
    }

    impl CoverSource for TestChain {
        async fn compact_blocks(
            &mut self,
            block_range: Range<BlockHeight>,
        ) -> Result<Vec<CompactBlock>, tonic::Status> {
            let block_range = u32::from(block_range.start)..u32::from(block_range.end);
            self.block_requests.push(block_range.clone());
            if let Some(code) = self.block_failure {
                return Err(tonic::Status::new(code, "the block range is not served"));
            }
            let block_range = block_range.start
                ..self
                    .served_blocks_end
                    .map_or(block_range.end, |end| end.min(block_range.end));

            Ok(block_range
                .map(|height| CompactBlock {
                    height: u64::from(height),
                    hash: vec![u8::from(self.replaced_blocks.contains(&height))],
                    time: height,
                    vtx: block_txids(height..height + 1)
                        .iter()
                        .enumerate()
                        .map(|(index, txid)| CompactTx {
                            txid: txid.as_ref().to_vec(),
                            actions: if index == 0 {
                                vec![CompactOrchardAction::default(); self.outputs_per_block]
                            } else {
                                Vec::new()
                            },
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                })
                .collect())
        }

        async fn transaction(&mut self, txid: TxId) -> Result<RawTransaction, tonic::Status> {
            self.transaction_requests.push(txid);
            if self.unserved.contains(&txid) {
                Err(tonic::Status::not_found("no such transaction"))
            } else {
                Ok(RawTransaction {
                    data: txid.as_ref().to_vec(),
                    height: self.served_height,
                })
            }
        }
    }

    fn request(
        targets: &[(u32, TxId)],
        scanned_heights: Range<u32>,
        chain_height: u32,
    ) -> CoverRequest {
        CoverRequest {
            targets: targets
                .iter()
                .map(|(height, txid)| (BlockHeight::from_u32(*height), *txid))
                .collect(),
            scanned_heights: BlockHeight::from_u32(scanned_heights.start)
                ..BlockHeight::from_u32(scanned_heights.end),
            timed_heights: Vec::new(),
            lowest_height: LOWEST_HEIGHT,
            chain_height: BlockHeight::from_u32(chain_height),
        }
    }

    async fn fetch_on(
        chain: &mut TestChain,
        cache: &mut CoverCache,
        request: CoverRequest,
    ) -> CoverFetch {
        fetch(chain, cache, request)
            .await
            .expect("the test chain serves every block")
    }

    #[test]
    fn a_group_closes_at_the_output_count() {
        // a group closes at the block that reaches the count, and the last blocks stay open
        assert_eq!(
            group_blocks(&[100, 28, 0, 200, 5, 5], false),
            (vec![0..2, 2..4], 4..6)
        );
        // in a complete area the last blocks join the last group
        assert_eq!(
            group_blocks(&[100, 28, 0, 200, 5, 5], true),
            (vec![0..2, 2..6], 6..6)
        );
        // a complete area below the count is one group
        let (closed, open) = group_blocks(&[5, 5, 5], true);
        assert_eq!(
            (closed.len(), closed.first(), open),
            (1, Some(&(0..3)), 3..3)
        );
        assert_eq!(group_blocks(&[5, 5, 5], false), (Vec::new(), 0..3));
    }

    #[tokio::test]
    async fn targets_in_one_group_send_identical_requests() {
        let mut request_sequences = Vec::new();
        for target in [(2_002, txid(2_002, 1)), (2_003, txid(2_003, 2))] {
            // 64 outputs in each block: every group is two blocks
            let mut chain = TestChain {
                outputs_per_block: 64,
                ..Default::default()
            };
            let fetched = fetch_on(
                &mut chain,
                &mut CoverCache::default(),
                request(&[target], 2_002..2_004, 5_000),
            )
            .await;

            assert!(matches!(fetched.target_replies.get(&target.1), Some(Ok(_))));
            assert_eq!(fetched.target_replies.len(), 1);
            assert_eq!(chain.block_requests, vec![2_000..3_000]);
            request_sequences.push(chain.transaction_requests);
        }

        assert_eq!(request_sequences[0], block_txids(2_002..2_004));
        assert_eq!(request_sequences[0], request_sequences[1]);
    }

    #[tokio::test]
    async fn open_group_blocks_are_requested_with_or_without_a_target() {
        // 50 outputs in each block: groups of three blocks from 2000, and block 2009 is open
        let mut request_sequences = Vec::new();
        for targets in [vec![(2_009, txid(2_009, 2))], Vec::new()] {
            let mut chain = TestChain {
                outputs_per_block: 50,
                ..Default::default()
            };
            fetch_on(
                &mut chain,
                &mut CoverCache::default(),
                request(&targets, 2_004..2_010, 2_009),
            )
            .await;

            assert_eq!(chain.block_requests, vec![2_000..2_010]);
            request_sequences.push(chain.transaction_requests);
        }

        assert_eq!(request_sequences[0], block_txids(2_009..2_010));
        assert_eq!(request_sequences[0], request_sequences[1]);
    }

    #[tokio::test]
    async fn a_group_is_requested_once_in_a_session() {
        let mut chain = TestChain {
            outputs_per_block: 64,
            ..Default::default()
        };
        let mut cache = CoverCache::default();
        fetch_on(
            &mut chain,
            &mut cache,
            request(&[(2_002, txid(2_002, 1))], 0..0, 5_000),
        )
        .await;

        let fetched = fetch_on(
            &mut chain,
            &mut cache,
            request(&[(2_003, txid(2_003, 2))], 0..0, 5_000),
        )
        .await;

        assert!(matches!(
            fetched.target_replies.get(&txid(2_003, 2)),
            Some(Ok(_))
        ));
        assert_eq!(chain.block_requests, vec![2_000..3_000]);
        assert_eq!(chain.transaction_requests, block_txids(2_002..2_004));
    }

    #[tokio::test]
    async fn a_failed_cover_request_is_dropped_and_a_failed_target_request_is_kept() {
        let cover = txid(2_002, 1);
        let target = txid(2_003, 2);
        // the block at the height of this target does not hold it
        let stray_target = txid(9, 9);
        let mut chain = TestChain {
            outputs_per_block: 64,
            unserved: vec![cover, target],
            ..Default::default()
        };

        let fetched = fetch_on(
            &mut chain,
            &mut CoverCache::default(),
            request(&[(2_003, target), (2_003, stray_target)], 0..0, 5_000),
        )
        .await;

        let mut expected_requests = block_txids(2_002..2_004);
        expected_requests.push(stray_target);
        assert_eq!(chain.transaction_requests, expected_requests);
        assert_eq!(fetched.target_replies.len(), 2);
        assert!(matches!(
            fetched.target_replies.get(&target),
            Some(Err(status)) if status.code() == tonic::Code::NotFound
        ));
        assert!(matches!(
            fetched.target_replies.get(&stray_target),
            Some(Ok(_))
        ));
    }

    #[tokio::test]
    async fn a_replaced_tip_block_reloads_the_area() {
        let mut chain = TestChain::default();
        let mut cache = CoverCache::default();
        fetch_on(&mut chain, &mut cache, request(&[], 2_005..2_006, 2_005)).await;
        fetch_on(&mut chain, &mut cache, request(&[], 2_006..2_007, 2_006)).await;
        chain.replaced_blocks = vec![2_006];
        fetch_on(&mut chain, &mut cache, request(&[], 2_007..2_008, 2_007)).await;

        assert_eq!(
            chain.block_requests,
            vec![2_000..2_006, 2_005..2_007, 2_006..2_008, 2_000..2_008]
        );
        // each new tip block is open and requested once
        assert_eq!(chain.transaction_requests, block_txids(2_005..2_008));
    }

    fn downloaded_blocks(chain: &TestChain) -> usize {
        chain
            .block_requests
            .iter()
            .map(ExactSizeIterator::len)
            .sum()
    }

    #[tokio::test]
    async fn finding_1_a_failed_area_download_still_answers_the_target() {
        let target = txid(2_003, 2);
        let mut chain = TestChain {
            outputs_per_block: 64,
            block_failure: Some(tonic::Code::OutOfRange),
            ..Default::default()
        };

        let fetched = fetch(
            &mut chain,
            &mut CoverCache::default(),
            request(&[(2_003, target)], 0..0, 5_000),
        )
        .await;

        assert!(
            matches!(&fetched, Ok(fetched) if fetched.target_replies.contains_key(&target)),
            "the fetch answered {fetched:?}"
        );
    }

    #[tokio::test]
    async fn finding_2_a_complete_cached_area_is_checked_again_after_a_reorg() {
        let mut chain = TestChain {
            outputs_per_block: 64,
            ..Default::default()
        };
        let mut cache = CoverCache::default();
        fetch_on(
            &mut chain,
            &mut cache,
            request(&[(2_990, txid(2_990, 1))], 0..0, 3_000),
        )
        .await;
        assert_eq!(chain.block_requests, vec![2_000..3_000]);

        chain.replaced_blocks = vec![2_999];
        fetch_on(
            &mut chain,
            &mut cache,
            request(&[(2_998, txid(2_998, 1))], 0..0, 3_000),
        )
        .await;

        assert!(
            chain.block_requests.len() > 1,
            "no block request followed the re-org of block 2999, one block below the chain tip"
        );
    }

    #[tokio::test]
    async fn finding_3_a_target_in_an_unloaded_block_is_not_requested_alone() {
        let target = txid(2_700, 1);
        let mut chain = TestChain {
            outputs_per_block: 64,
            served_blocks_end: Some(2_500),
            ..Default::default()
        };

        fetch_on(
            &mut chain,
            &mut CoverCache::default(),
            request(&[(2_700, target)], 0..0, 5_000),
        )
        .await;

        assert_ne!(chain.transaction_requests, vec![target]);
    }

    #[tokio::test]
    async fn finding_4_a_block_time_downloads_one_block() {
        let mut chain = TestChain::default();
        let mut block_time_request = request(&[], 0..0, 5_000);
        block_time_request.timed_heights = vec![BlockHeight::from_u32(2_500)];

        let fetched = fetch_on(&mut chain, &mut CoverCache::default(), block_time_request).await;

        assert_eq!(
            fetched.block_times.get(&BlockHeight::from_u32(2_500)),
            Some(&2_500)
        );
        assert_eq!(downloaded_blocks(&chain), 1);
    }

    #[tokio::test]
    async fn finding_4_interleaved_areas_are_downloaded_once_each() {
        let area_starts = [2_000, 3_000, 4_000, 5_000, 6_000, 7_000];
        let mut chain = TestChain {
            outputs_per_block: 64,
            ..Default::default()
        };
        let mut cache = CoverCache::default();
        for _round in 0..2 {
            for area_start in area_starts {
                fetch_on(
                    &mut chain,
                    &mut cache,
                    request(&[(area_start + 2, txid(area_start + 2, 1))], 0..0, 50_000),
                )
                .await;
            }
        }

        assert_eq!(chain.block_requests.len(), area_starts.len());
    }

    #[tokio::test]
    async fn finding_4_a_scan_below_the_tip_area_sends_no_request() {
        let mut chain = TestChain::default();

        fetch_on(
            &mut chain,
            &mut CoverCache::default(),
            request(&[], 100..200, 5_000),
        )
        .await;

        assert!(chain.block_requests.is_empty());
        assert!(chain.transaction_requests.is_empty());
    }

    #[tokio::test]
    async fn finding_6_a_cached_transaction_is_not_served_after_its_area_reloads() {
        let target = txid(2_002, 1);
        let mut chain = TestChain {
            outputs_per_block: 64,
            served_height: 2_002,
            ..Default::default()
        };
        let mut cache = CoverCache::default();
        fetch_on(
            &mut chain,
            &mut cache,
            request(&[(2_002, target)], 0..0, 2_005),
        )
        .await;

        chain.replaced_blocks = vec![2_005];
        chain.served_height = 2_003;
        let fetched = fetch_on(
            &mut chain,
            &mut cache,
            request(&[(2_002, target)], 0..0, 2_005),
        )
        .await;

        assert_eq!(chain.block_requests.last(), Some(&(2_000..2_006)));
        assert!(
            matches!(
                fetched.target_replies.get(&target),
                Some(Ok(raw_transaction)) if raw_transaction.height == 2_003
            ),
            "the reply after the reload was {:?}",
            fetched.target_replies.get(&target)
        );
    }
}

//! Cover ranges for transaction requests.
//!
//! A cover range is a fixed, aligned range of blocks. To fetch one
//! transaction, sync requests every transaction in its range.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    ops::Range,
};

use tokio::sync::mpsc::UnboundedSender;
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;
use zl_netutils::lightwallet_protocol::RawTransaction;

use crate::{
    client::{self, FetchRequest},
    error::ServerError,
    utils::transaction::get_compact_txid,
};

// The length of one cover range
pub(crate) const COVER_RANGE_BLOCKS: u32 = 10;

/// Returns the cover range that contains `height`.
///
/// A range cut short by the chain tip is extended down by one range.
fn cover_range(
    height: BlockHeight,
    lowest_height: Option<BlockHeight>,
    chain_height: BlockHeight,
) -> Range<BlockHeight> {
    let aligned_start = height - u32::from(height) % COVER_RANGE_BLOCKS;
    let aligned_end = aligned_start + COVER_RANGE_BLOCKS;
    let chain_end = chain_height.max(height) + 1;
    let start = if aligned_end > chain_end {
        aligned_start.saturating_sub(COVER_RANGE_BLOCKS)
    } else {
        aligned_start
    };

    lowest_height.map_or(start, |lowest_height| start.max(lowest_height))
        ..aligned_end.min(chain_end)
}

/// Returns every block height in the cover ranges of `target_heights`.
fn cover_heights(
    target_heights: impl Iterator<Item = BlockHeight>,
    lowest_height: Option<BlockHeight>,
    chain_height: BlockHeight,
) -> BTreeSet<BlockHeight> {
    target_heights
        .flat_map(|height| {
            let range = cover_range(height, lowest_height, chain_height);
            (u32::from(range.start)..u32::from(range.end)).map(BlockHeight::from_u32)
        })
        .collect()
}

/// The result of [`fetch_under_cover`].
pub(crate) struct CoverFetch {
    pub(crate) target_replies: HashMap<TxId, Result<RawTransaction, ServerError>>,
    pub(crate) block_times: HashMap<BlockHeight, u32>,
}

/// Fetches `targets` together with every transaction in their cover ranges.
///
/// Also fetches the block times of the cover ranges of `timed_heights`.
pub(crate) async fn fetch_under_cover(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    targets: &[(BlockHeight, TxId)],
    timed_heights: &[BlockHeight],
    lowest_height: Option<BlockHeight>,
    chain_height: BlockHeight,
) -> Result<CoverFetch, ServerError> {
    let transaction_heights = cover_heights(
        targets.iter().map(|(height, _)| *height),
        lowest_height,
        chain_height,
    );
    let block_heights = transaction_heights
        .iter()
        .copied()
        .chain(cover_heights(
            timed_heights.iter().copied(),
            lowest_height,
            chain_height,
        ))
        .collect::<BTreeSet<_>>();
    let mut cover_txids = Vec::new();
    let mut block_times = HashMap::with_capacity(block_heights.len());
    for height in block_heights {
        let compact_block = client::get_compact_block(fetch_request_sender.clone(), height).await?;
        block_times.insert(height, compact_block.time);
        if transaction_heights.contains(&height) {
            cover_txids.extend(compact_block.vtx.iter().map(get_compact_txid));
        }
    }

    let target_txids = targets
        .iter()
        .map(|(_, txid)| *txid)
        .collect::<HashSet<_>>();
    let mut requested = HashSet::new();
    let mut target_replies = HashMap::with_capacity(target_txids.len());
    for txid in cover_txids
        .into_iter()
        .chain(targets.iter().map(|(_, txid)| *txid))
    {
        if !requested.insert(txid) {
            continue;
        }

        let reply = client::get_raw_transaction(fetch_request_sender.clone(), txid).await;
        if target_txids.contains(&txid) {
            target_replies.insert(txid, reply);
        }
    }

    Ok(CoverFetch {
        target_replies,
        block_times,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::sync::mpsc;
    use zl_netutils::lightwallet_protocol::{CompactBlock, CompactTx};

    use super::*;

    const LOWEST_HEIGHT: Option<BlockHeight> = Some(BlockHeight::from_u32(1));

    fn txid(block_height: u32, index: u8) -> TxId {
        let mut bytes = [index; 32];
        bytes[..4].copy_from_slice(&block_height.to_le_bytes());
        TxId::from_bytes(bytes)
    }

    fn block_txids(block_height: u32) -> [TxId; 2] {
        [txid(block_height, 1), txid(block_height, 2)]
    }

    fn recording_fetcher(
        unserved: Vec<TxId>,
    ) -> (mpsc::UnboundedSender<FetchRequest>, Arc<Mutex<Vec<TxId>>>) {
        let (fetch_request_sender, mut fetch_request_receiver) = mpsc::unbounded_channel();
        let requested = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requested);
        tokio::spawn(async move {
            while let Some(fetch_request) = fetch_request_receiver.recv().await {
                match fetch_request {
                    FetchRequest::CompactBlock(reply_sender, block_height) => {
                        let height = u32::from(block_height);
                        let _ignore_error = reply_sender.send(Ok(CompactBlock {
                            height: u64::from(height),
                            time: height,
                            vtx: block_txids(height)
                                .iter()
                                .map(|txid| CompactTx {
                                    txid: txid.as_ref().to_vec(),
                                    ..Default::default()
                                })
                                .collect(),
                            ..Default::default()
                        }));
                    }
                    FetchRequest::Transaction(reply_sender, txid) => {
                        log.lock()
                            .expect("no holder of the request log panics")
                            .push(txid);
                        let reply = if unserved.contains(&txid) {
                            Err(tonic::Status::not_found("no such transaction"))
                        } else {
                            Ok(RawTransaction {
                                data: txid.as_ref().to_vec(),
                                height: 0,
                            })
                        };
                        let _ignore_error = reply_sender.send(reply);
                    }
                    _ => panic!("unexpected fetch request"),
                }
            }
        });

        (fetch_request_sender, requested)
    }

    fn requested_txids(requested: &Arc<Mutex<Vec<TxId>>>) -> Vec<TxId> {
        requested
            .lock()
            .expect("no holder of the request log panics")
            .clone()
    }

    fn range_txids(block_heights: Range<u32>) -> Vec<TxId> {
        block_heights.flat_map(block_txids).collect()
    }

    #[test]
    fn cover_range_is_aligned_and_bounded_by_the_chain() {
        let range = |height, chain_height| {
            let range = cover_range(
                BlockHeight::from_u32(height),
                LOWEST_HEIGHT,
                BlockHeight::from_u32(chain_height),
            );
            u32::from(range.start)..u32::from(range.end)
        };

        for height in 100..110 {
            assert_eq!(range(height, 1_000), 100..110);
        }
        assert_eq!(range(105, 109), 100..110);
        assert_eq!(range(100, 100), 90..101);
        assert_eq!(range(3, 5), 1..6);
    }

    #[tokio::test]
    async fn targets_in_one_range_send_identical_requests() {
        let mut request_sequences = Vec::new();
        for (block_height, target) in [(102, txid(102, 1)), (107, txid(107, 2))] {
            let (fetch_request_sender, requested) = recording_fetcher(Vec::new());
            let fetched = fetch_under_cover(
                fetch_request_sender,
                &[(BlockHeight::from_u32(block_height), target)],
                &[],
                LOWEST_HEIGHT,
                BlockHeight::from_u32(1_000),
            )
            .await
            .expect("the fetcher answers every compact block request");

            assert_eq!(
                fetched.target_replies.keys().copied().collect::<Vec<_>>(),
                vec![target]
            );
            request_sequences.push(requested_txids(&requested));
        }

        assert_eq!(request_sequences[0], range_txids(100..110));
        assert_eq!(request_sequences[0], request_sequences[1]);
    }

    #[tokio::test]
    async fn a_failed_cover_request_is_dropped_and_a_failed_target_request_is_kept() {
        let cover = txid(101, 1);
        let target = txid(104, 2);
        let (fetch_request_sender, requested) = recording_fetcher(vec![cover, target]);

        let fetched = fetch_under_cover(
            fetch_request_sender,
            &[(BlockHeight::from_u32(104), target)],
            &[],
            LOWEST_HEIGHT,
            BlockHeight::from_u32(1_000),
        )
        .await
        .expect("the fetcher answers every compact block request");

        assert_eq!(requested_txids(&requested), range_txids(100..110));
        assert_eq!(fetched.target_replies.len(), 1);
        assert!(matches!(
            fetched.target_replies.get(&target),
            Some(Err(ServerError::RequestFailed(status))) if status.code() == tonic::Code::NotFound
        ));
    }

    #[tokio::test]
    async fn overlapping_ranges_request_each_transaction_once_and_a_stray_target_last() {
        let stray_target = txid(500, 1);
        let (fetch_request_sender, requested) = recording_fetcher(Vec::new());

        let fetched = fetch_under_cover(
            fetch_request_sender,
            &[
                (BlockHeight::from_u32(95), txid(95, 1)),
                (BlockHeight::from_u32(101), txid(101, 1)),
                (BlockHeight::from_u32(101), stray_target),
            ],
            &[],
            LOWEST_HEIGHT,
            BlockHeight::from_u32(102),
        )
        .await
        .expect("the fetcher answers every compact block request");

        let mut expected_requests = range_txids(90..103);
        expected_requests.push(stray_target);
        assert_eq!(requested_txids(&requested), expected_requests);
        assert!(matches!(
            fetched.target_replies.get(&stray_target),
            Some(Ok(_))
        ));
    }
}

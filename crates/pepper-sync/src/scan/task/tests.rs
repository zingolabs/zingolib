use zcash_protocol::consensus::{MAIN_NETWORK, MainNetwork};
use zcash_transparent::keys::NonHardenedChildIndex;

use crate::keys::transparent::TransparentScope;

use super::*;

const GAP_LIMIT: u32 = 3;
const AWAITED_HEIGHT: u32 = 120;
const OTHER_HEIGHT: u32 = 110;

fn load_key(task_id: TaskId, height: u32) -> LoadKey {
    (task_id, BlockHeight::from_u32(height))
}

#[test]
fn a_running_scan_task_runner_that_awaits_no_arrival_is_idle() {
    assert_eq!(
        ScanTaskRunnerActivity::of(false, false),
        ScanTaskRunnerActivity::Idle
    );
}

#[test]
fn a_running_scan_task_runner_that_awaits_an_arrival_is_busy() {
    assert_eq!(
        ScanTaskRunnerActivity::of(true, false),
        ScanTaskRunnerActivity::Busy
    );
}

#[test]
fn an_ended_scan_task_runner_is_ended_whatever_it_awaits() {
    assert_eq!(
        ScanTaskRunnerActivity::of(false, true),
        ScanTaskRunnerActivity::Ended
    );
    assert_eq!(
        ScanTaskRunnerActivity::of(true, true),
        ScanTaskRunnerActivity::Ended
    );
}

#[test]
fn the_awaited_arrival_ends_the_wait() {
    let awaited = load_key(TaskId::first(), AWAITED_HEIGHT);
    assert_eq!(awaited_after(Some(awaited), &awaited), None);
}

#[test]
fn an_arrival_at_another_height_keeps_the_wait() {
    let awaited = load_key(TaskId::first(), AWAITED_HEIGHT);
    let arrived = load_key(TaskId::first(), OTHER_HEIGHT);
    assert_eq!(awaited_after(Some(awaited), &arrived), Some(awaited));
}

#[test]
fn an_arrival_of_another_scan_task_keeps_the_wait() {
    let awaited = load_key(TaskId::first(), AWAITED_HEIGHT);
    let arrived = load_key(TaskId::first().next(), AWAITED_HEIGHT);
    assert_eq!(awaited_after(Some(awaited), &arrived), Some(awaited));
}

#[test]
fn an_arrival_with_nothing_awaited_leaves_nothing_awaited() {
    let arrived = load_key(TaskId::first(), AWAITED_HEIGHT);
    assert_eq!(awaited_after(None, &arrived), None);
}

/// An address map as held by the scanner, for the given external indexes.
fn external_addresses(
    indexes: impl IntoIterator<Item = u32>,
) -> HashMap<String, TransparentAddressId> {
    indexes
        .into_iter()
        .map(|index| {
            (
                format!("external address {index}"),
                TransparentAddressId::new(
                    AccountId::ZERO,
                    TransparentScope::External,
                    NonHardenedChildIndex::from_index(index).unwrap(),
                ),
            )
        })
        .collect()
}

fn scanner_with_gap_addresses(
    gap_addresses: HashMap<String, TransparentAddressId>,
) -> Scanner<MainNetwork> {
    let (scan_results_sender, _) = mpsc::unbounded_channel();
    let (fetch_request_sender, _) = mpsc::unbounded_channel();
    let mut scanner = Scanner::new(
        MAIN_NETWORK,
        scan_results_sender,
        fetch_request_sender,
        HashMap::new(),
        GAP_LIMIT,
    );
    scanner.transparent_gap_addresses = gap_addresses;
    scanner
}

/// Scan results without compact block transparent data, such as re-fetched nullifiers, carry no changes to the
/// gap addresses. The scanner keeps the gap addresses it holds.
#[test]
fn gap_addresses_are_kept_when_scan_results_carry_no_changes() {
    let mut scanner = scanner_with_gap_addresses(external_addresses(1..=3));

    scanner.update_transparent_gap_addresses(&HashMap::new(), HashMap::new());

    assert_eq!(scanner.transparent_gap_addresses, external_addresses(1..=3));
}

/// The gap addresses found in use are removed and the gap addresses derived to replace them are added. The gap
/// addresses above the highest address found in use are kept.
#[test]
fn gap_addresses_found_in_use_are_replaced() {
    let mut scanner = scanner_with_gap_addresses(external_addresses(1..=3));

    scanner.update_transparent_gap_addresses(&external_addresses(1..=2), external_addresses(4..=5));

    assert_eq!(scanner.transparent_gap_addresses, external_addresses(3..=5));
}

const SCAN_RANGE_END: u32 = 20;

fn block_range() -> Range<BlockHeight> {
    BlockHeight::from_u32(10)..BlockHeight::from_u32(SCAN_RANGE_END + 1)
}

fn no_block_error() -> ServerError {
    ServerError::RequestFailed(tonic::Status::not_found("no block"))
}

/// Answers chain height requests with `chain_height`.
fn spawn_chain_height_fetcher(chain_height: u32) -> mpsc::UnboundedSender<FetchRequest> {
    let (fetch_request_sender, mut fetch_request_receiver) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(fetch_request) = fetch_request_receiver.recv().await {
            match fetch_request {
                FetchRequest::ChainTip(reply_sender) => {
                    let _ignore_error =
                        reply_sender.send(Ok(zingolib_netutils::lightwallet_protocol::BlockId {
                            height: u64::from(chain_height),
                            hash: Vec::new(),
                        }));
                }
                _ => panic!("unexpected fetch request"),
            }
        }
    });

    fetch_request_sender
}

/// The server's chain height is below the last block of the scan range, so it has no block to serve for the top
/// of the scan range.
#[tokio::test]
async fn fetch_failure_under_a_lowered_chain_height_reports_the_chain_height() {
    let error = fetch_failure(
        spawn_chain_height_fetcher(SCAN_RANGE_END - 1),
        &block_range(),
        no_block_error(),
    )
    .await;

    assert!(matches!(
        error,
        ServerError::ChainHeightBelowScanRange { chain_height, scan_range_end }
            if chain_height == BlockHeight::from_u32(SCAN_RANGE_END - 1)
                && scan_range_end == BlockHeight::from_u32(SCAN_RANGE_END)
    ));
}

/// The server's chain holds the whole scan range, so the fetch failed for another reason.
#[tokio::test]
async fn fetch_failure_with_the_scan_range_on_chain_returns_the_fetch_error() {
    for chain_height in [SCAN_RANGE_END, SCAN_RANGE_END + 1] {
        let error = fetch_failure(
            spawn_chain_height_fetcher(chain_height),
            &block_range(),
            no_block_error(),
        )
        .await;

        assert!(
            matches!(error, ServerError::RequestFailed(_)),
            "chain height {chain_height}"
        );
    }
}

/// The chain height request fails with the fetcher gone, leaving the fetch error as the only known cause.
#[tokio::test]
async fn fetch_failure_with_no_chain_height_returns_the_fetch_error() {
    let (fetch_request_sender, _) = mpsc::unbounded_channel();

    let error = fetch_failure(fetch_request_sender, &block_range(), no_block_error()).await;

    assert!(matches!(error, ServerError::RequestFailed(_)));
}

fn load(task_id: TaskId, start: u32, end: u32) -> ScanLoad {
    ScanLoad {
        task_id,
        scan_range: ScanRange::from_parts(
            BlockHeight::from_u32(start)..BlockHeight::from_u32(end),
            ScanPriority::ChainTip,
        ),
        chain_height: BlockHeight::from_u32(end - 1),
        highest_scanned_height: BlockHeight::from_u32(start - 1),
    }
}

fn results() -> Result<ScanResults, ScanError> {
    Err(ScanError::ServerError(no_block_error()))
}

fn released_starts(released: &[(ScanLoad, Result<ScanResults, ScanError>)]) -> Vec<u32> {
    released
        .iter()
        .map(|(load, _)| u32::from(load.scan_range.block_range().start))
        .collect()
}

/// Loads of a task arriving in block height order are released as they arrive.
#[test]
fn loads_arriving_in_order_are_released_as_they_arrive() {
    let task = TaskId::first();
    let mut order = LoadOrder::default();
    order.track(task, BlockHeight::from_u32(10));

    assert_eq!(
        released_starts(&order.admit(load(task, 10, 20), results())),
        [10]
    );
    assert_eq!(
        released_starts(&order.admit(load(task, 20, 30), results())),
        [20]
    );
    assert_eq!(order.held(), 0);
}

/// A load arriving above an unreleased lower load of its task is held back, and released in height order once
/// the lower load arrives.
#[test]
fn a_load_above_an_unreleased_load_is_held_until_the_lower_load_arrives() {
    let task = TaskId::first();
    let mut order = LoadOrder::default();
    order.track(task, BlockHeight::from_u32(10));

    assert!(order.admit(load(task, 30, 40), results()).is_empty());
    assert!(order.admit(load(task, 20, 30), results()).is_empty());
    assert_eq!(order.held(), 2);
    assert_eq!(
        released_starts(&order.admit(load(task, 10, 20), results())),
        [10, 20, 30]
    );
    assert_eq!(order.held(), 0);
}

/// The ordering of one task does not hold back the loads of another.
#[test]
fn tasks_are_ordered_independently() {
    let first = TaskId::first();
    let second = first.next();
    let mut order = LoadOrder::default();
    order.track(first, BlockHeight::from_u32(10));
    order.track(second, BlockHeight::from_u32(50));

    assert!(order.admit(load(first, 20, 30), results()).is_empty());
    assert_eq!(
        released_starts(&order.admit(load(second, 50, 60), results())),
        [50]
    );
}

/// The results of a task the scanner does not track are released as they are.
#[test]
fn loads_of_an_untracked_task_are_released_as_they_are() {
    let mut order = LoadOrder::default();

    assert_eq!(
        released_starts(&order.admit(load(TaskId::first(), 20, 30), results())),
        [20]
    );
    assert_eq!(order.held(), 0);
}

/// Retiring a task drops the results held back for it.
#[test]
fn retiring_a_task_drops_its_held_results() {
    let task = TaskId::first();
    let mut order = LoadOrder::default();
    order.track(task, BlockHeight::from_u32(10));
    assert!(order.admit(load(task, 20, 30), results()).is_empty());

    order.retain(&BTreeMap::new());

    assert_eq!(order.held(), 0);
    assert_eq!(
        released_starts(&order.admit(load(task, 10, 20), results())),
        [10]
    );
}

/// A held result takes the place of a load of compact blocks in a worker, so a worker takes a load only while
/// the scanning workers and held results together stay within the pool.
#[test]
fn a_worker_takes_a_load_only_within_the_pool() {
    for (scanning_workers, held_results, admitted) in [
        (0, 0, true),
        (MAX_WORKER_POOLSIZE - 1, 0, true),
        (0, MAX_WORKER_POOLSIZE - 1, true),
        (MAX_WORKER_POOLSIZE - 1, 1, false),
        (0, MAX_WORKER_POOLSIZE, false),
    ] {
        assert_eq!(
            load_admitted(scanning_workers, held_results),
            admitted,
            "{scanning_workers} scanning, {held_results} held"
        );
    }
}

/// A scan task of any priority is split above its first block when a load budget is reached.
#[test]
fn scan_tasks_are_split_above_their_first_block() {
    const START: u32 = 10;
    let block_range = BlockHeight::from_u32(START)..BlockHeight::from_u32(START * 2);

    for (priority, block_height, splittable) in [
        (ScanPriority::ChainTip, START, false),
        (ScanPriority::ChainTip, START + 1, true),
        (ScanPriority::ScannedWithoutMapping, START + 1, true),
        (ScanPriority::Verify, START, false),
        (ScanPriority::Verify, START + 1, true),
    ] {
        assert_eq!(
            splittable_at(
                &ScanRange::from_parts(block_range.clone(), priority),
                BlockHeight::from_u32(block_height)
            ),
            splittable,
            "{priority:?} at {block_height}"
        );
    }
}

#[test]
fn transparent_data_is_only_requested_for_ranges_above_transparent_scan_floor() {
    let floor = BlockHeight::from_u32(1_000);
    for (block_range, include_transparent) in [
        // below the floor
        (floor - 10..floor - 5, false),
        // ends at the floor
        (floor - 5..floor + 1, false),
        // spans the floor
        (floor - 5..floor + 5, true),
        // starts above the floor
        (floor + 1..floor + 5, true),
    ] {
        assert_eq!(
            includes_blocks_above_floor(&block_range, floor),
            include_transparent,
            "{block_range:?}"
        );
    }
}

/// A load budget splits a scan task of any priority, since the re-org handler resets the wallet range that
/// encloses the failed load instead of the load's exact range.
#[test]
fn a_verify_task_splits_at_a_load_budget_like_any_other() {
    let verify = ScanRange::from_parts(
        BlockHeight::from_u32(21)..BlockHeight::from_u32(41),
        ScanPriority::Verify,
    );

    assert!(splittable_at(&verify, BlockHeight::from_u32(31)));
    assert!(!splittable_at(&verify, BlockHeight::from_u32(21)));
}

//! Entrypoint for sync engine

use std::ops::Range;

use crate::client::FetchRequest;
use crate::client::{fetch::fetch, get_chain_height};
use crate::interface::{SyncBlocks, SyncNullifiers, SyncShardTrees, SyncWallet};
use crate::keys::ScanningKeys;
use crate::primitives::SyncState;
use crate::scan::{scan, ScanData};

use tokio::task::JoinHandle;
use zcash_client_backend::{
    data_api::scanning::{ScanPriority, ScanRange},
    proto::service::compact_tx_streamer_client::CompactTxStreamerClient,
};

use futures::future::try_join_all;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use zcash_primitives::consensus::{BlockHeight, NetworkUpgrade, Parameters};

const SCAN_WORKERS: usize = 2;
const BATCH_SIZE: u32 = 10;
// const BATCH_SIZE: u32 = 1_000;

/// Syncs a wallet to the latest state of the blockchain
pub async fn sync<P, W>(
    client: CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
    parameters: &P,
    wallet: &mut W,
) -> Result<(), ()>
where
    P: Parameters + Sync + Send + 'static,
    W: SyncWallet + SyncBlocks + SyncNullifiers + SyncShardTrees,
{
    tracing::info!("Syncing wallet...");

    // TODO: add trait methods to read/write wallet data to/from sync engine
    // this is where data would be read from wallet
    let sync_state = SyncState::new(); // placeholders

    // create channel for sending fetch requests and launch fetcher task
    let (fetch_request_sender, fetch_request_receiver) = unbounded_channel();
    let fetcher_handle = tokio::spawn(fetch(fetch_request_receiver, client));

    update_scan_ranges(fetch_request_sender.clone(), parameters, &sync_state)
        .await
        .unwrap();

    // let scan_handle_a = run_scan_task(
    //     fetch_request_sender.clone(),
    //     parameters.clone(),
    //     wallet,
    //     &sync_state,
    // )
    // .await;

    // if let Some(handle) = scan_handle_a {
    //     let scan_data = handle.await.unwrap().unwrap();

    //     let ScanData {
    //         nullifiers,
    //         wallet_blocks,
    //         shard_tree_data,
    //     } = scan_data;

    //     // TODO: if scan priority is historic, retain only relevent blocks and nullifiers as we have all information and requires a lot of memory / storage
    //     // must still retain top 100 blocks for re-org purposes
    //     wallet.append_wallet_blocks(wallet_blocks).unwrap();
    //     wallet.append_nullifiers(nullifiers).unwrap();
    //     wallet.update_shard_trees(shard_tree_data).unwrap();
    //     // TODO: add trait to save wallet data to persistance for in-memory wallets

    //     // TODO: set scanned range to `scanned`
    // }

    drop(fetch_request_sender);
    try_join_all(vec![fetcher_handle]).await.unwrap();

    Ok(())
}

// update scan_ranges to include blocks between the last known chain height (wallet height) and the chain height from the server
async fn update_scan_ranges<P>(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    parameters: &P,
    sync_state: &SyncState,
) -> Result<(), ()>
where
    P: Parameters,
{
    let chain_height = get_chain_height(fetch_request_sender).await.unwrap();

    let mut scan_ranges = sync_state.scan_ranges().write().await;

    let wallet_height = if scan_ranges.is_empty() {
        // TODO: add birthday
        parameters
            .activation_height(NetworkUpgrade::Sapling)
            .expect("sapling activation height should always return Some")
    } else {
        scan_ranges
            .last()
            .expect("Vec should not be empty")
            .block_range()
            .end
    };

    if wallet_height > chain_height {
        panic!("wallet is ahead of server!")
    }

    let chain_tip_scan_range = ScanRange::from_parts(
        Range {
            start: wallet_height,
            end: chain_height + 1,
        },
        ScanPriority::Historic,
    );
    scan_ranges.push(chain_tip_scan_range);

    if scan_ranges.is_empty() {
        panic!("scan ranges should never be empty after updating")
    }

    // TODO: add logic to combine chain tip scan range with wallet tip scan range
    // TODO: add scan priority logic

    Ok(())
}

// returns `None` if there are no more ranges to scan
async fn prepare_next_scan_range(sync_state: &SyncState) -> Option<ScanRange> {
    let mut scan_ranges = sync_state.scan_ranges().write().await;

    // placeholder for algorythm that determines highest priority range to scan
    let (index, selected_scan_range) = scan_ranges.iter_mut().enumerate().find(|(_, range)| {
        range.priority() != ScanPriority::Scanned && range.priority() != ScanPriority::Ignored
    })?;

    // if scan range is larger than BATCH_SIZE, split off and return a batch from the lower end and update scan ranges
    if let Some((lower_range, higher_range)) = selected_scan_range
        .split_at(selected_scan_range.block_range().start + BlockHeight::from_u32(BATCH_SIZE))
    {
        let lower_range_ignored =
            ScanRange::from_parts(lower_range.block_range().clone(), ScanPriority::Ignored);
        scan_ranges.splice(index..=index, vec![lower_range_ignored, higher_range]);

        Some(lower_range)
    } else {
        let selected_scan_range = selected_scan_range.clone();
        let selected_range_ignored = ScanRange::from_parts(
            selected_scan_range.block_range().clone(),
            ScanPriority::Ignored,
        );
        scan_ranges.splice(index..=index, vec![selected_range_ignored]);

        Some(selected_scan_range.clone())
    }
}

async fn run_scan_task<P, W>(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    parameters: P,
    wallet: &mut W,
    sync_state: &SyncState, // temp
) -> Result<Option<JoinHandle<Result<ScanData, ()>>>, ()>
where
    P: Parameters + Sync + Send + 'static,
    W: SyncWallet + SyncBlocks + SyncNullifiers + SyncShardTrees,
{
    if let Some(scan_range) = prepare_next_scan_range(&sync_state).await {
        let previous_wallet_block = wallet
            .get_wallet_block(scan_range.block_range().start - 1)
            .ok();
        let ufvks = wallet.get_unified_full_viewing_keys().unwrap();
        let scanning_keys = ScanningKeys::from_account_ufvks(ufvks);
        Ok(Some(tokio::spawn(async move {
            scan(
                fetch_request_sender,
                &parameters,
                scan_range.clone(),
                &scanning_keys,
                previous_wallet_block,
            )
            .await
        })))
    } else {
        Ok(None)
    }
}

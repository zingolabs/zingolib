//! Entrypoint for sync engine

use std::ops::Range;

use crate::client::FetchRequest;
use crate::client::{fetch::fetch, get_chain_height};
use crate::interface::{SyncBlocks, SyncNullifiers, SyncShardTrees, SyncWallet};
use crate::primitives::SyncState;
use crate::scan::{scan, ScanData};

use zcash_client_backend::scanning::ScanningKeys;
use zcash_client_backend::{
    data_api::scanning::{ScanPriority, ScanRange},
    proto::service::compact_tx_streamer_client::CompactTxStreamerClient,
};

use futures::future::try_join_all;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use zcash_primitives::consensus::{BlockHeight, NetworkUpgrade, Parameters};

const BATCH_SIZE: u32 = 10;
// const BATCH_SIZE: u32 = 1_000;

/// Syncs a wallet to the latest state of the blockchain
pub async fn sync<P, W>(
    client: CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
    parameters: &P,
    wallet: &mut W,
) -> Result<(), ()>
where
    P: Parameters + Send + 'static,
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

    let account_ufvks = wallet.get_unified_full_viewing_keys().unwrap();
    let scanning_keys = ScanningKeys::from_account_ufvks(account_ufvks);

    let scan_range = prepare_next_scan_range(&sync_state).await;
    if let Some(range) = scan_range {
        let previous_wallet_block = wallet.get_wallet_block(range.block_range().start - 1).ok();
        let scan_data = scan(
            fetch_request_sender,
            parameters,
            &scanning_keys,
            range.clone(),
            previous_wallet_block,
        )
        .await
        .unwrap();

        let ScanData {
            nullifiers,
            wallet_blocks,
            shard_tree_data,
        } = scan_data;

        // TODO: if scan priority is historic, retain only relevent blocks and nullifiers as we have all information and requires a lot of memory / storage
        // must still retain top 100 blocks for re-org purposes
        wallet.append_wallet_blocks(wallet_blocks).unwrap();
        wallet.append_nullifiers(nullifiers).unwrap();
        wallet.update_shard_trees(shard_tree_data).unwrap();

        // TODO: set scanned range to `scanned`
    }

    // TODO: add trait to save wallet data to persistance for in-memory wallets

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
        scan_ranges.splice(index..=index, vec![lower_range.clone(), higher_range]);

        // TODO: add an `exclude` parameter so the same range is not selected twice when multiple tasks call this fn

        Some(lower_range)
    } else {
        // TODO: add an `exclude` parameter so the same range is not selected twice when multiple tasks call this fn

        Some(selected_scan_range.clone())
    }
}

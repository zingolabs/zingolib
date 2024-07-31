//! Entrypoint for sync engine

use std::ops::Range;

use crate::client::FetchRequest;
use crate::client::{fetcher::fetcher, get_chain_height};
use crate::interface::SyncWallet;
use crate::scanner::scanner;

use zcash_client_backend::{
    data_api::scanning::{ScanPriority, ScanRange},
    proto::service::compact_tx_streamer_client::CompactTxStreamerClient,
};

use futures::future::try_join_all;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use zcash_primitives::consensus::{BlockHeight, NetworkUpgrade, Parameters};

const BATCH_SIZE: u32 = 1_000;

/// Syncs a wallet to the latest state of the blockchain
pub async fn sync<P, W>(
    client: CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
    parameters: &P,
    wallet: &mut W,
) -> Result<(), ()>
where
    P: Parameters + Send + 'static,
    W: SyncWallet,
{
    tracing::info!("Syncing wallet...");

    // create channel for sending fetch requests and launch fetcher task
    let (fetch_request_sender, fetch_request_receiver) = unbounded_channel();
    let fetcher_handle = tokio::spawn(fetcher(fetch_request_receiver, client));

    update_scan_ranges(fetch_request_sender.clone(), parameters, wallet)
        .await
        .unwrap();

    let scan_range = prepare_next_scan_range(wallet);

    if let Some(range) = scan_range {
        scanner(fetch_request_sender, parameters, wallet, range.clone())
            .await
            .unwrap();
        // let scanner_handle = tokio::spawn(scanner(fetch_request_sender, wallet, range.clone()));
        // scanner_handle.await.unwrap().unwrap();
    }

    try_join_all(vec![fetcher_handle]).await.unwrap();

    Ok(())
}

// update scan_ranges to include blocks between the last known chain height (wallet height) and the chain height from the server
async fn update_scan_ranges<P, W>(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    parameters: &P,
    wallet_data: &mut W,
) -> Result<(), ()>
where
    P: Parameters,
    W: SyncWallet,
{
    let scan_ranges = wallet_data.set_sync_state().unwrap().set_scan_ranges();

    let chain_height = get_chain_height(fetch_request_sender).await.unwrap();
    let wallet_height = if scan_ranges.is_empty() {
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
fn prepare_next_scan_range<W>(wallet: &mut W) -> Option<ScanRange>
where
    W: SyncWallet,
{
    let scan_ranges = wallet.set_sync_state().unwrap().set_scan_ranges();

    // placeholder for algorythm that determines highest priority range to scan
    let (index, selected_scan_range) = scan_ranges.iter_mut().enumerate().find(|(_, range)| {
        range.priority() != ScanPriority::Scanned && range.priority() != ScanPriority::Ignored
    })?;

    // if scan range is larger than BATCH_SIZE, split off and return a batch from the lower end and update scan ranges
    if let Some((lower_range, higher_range)) = selected_scan_range
        .split_at(selected_scan_range.block_range().start + BlockHeight::from_u32(BATCH_SIZE))
    {
        scan_ranges.splice(index..=index, vec![lower_range.clone(), higher_range]);

        Some(lower_range)
    } else {
        Some(selected_scan_range.clone())
    }
}

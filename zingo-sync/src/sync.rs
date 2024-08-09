//! Entrypoint for sync engine

use std::ops::Range;

use crate::client::{fetcher::fetcher, get_chain_height};
use crate::interface::SyncWallet;

use zcash_client_backend::{
    data_api::scanning::{ScanPriority, ScanRange},
    proto::service::compact_tx_streamer_client::CompactTxStreamerClient,
};

use futures::future::try_join_all;
use tokio::sync::mpsc::unbounded_channel;
use zcash_primitives::consensus::{BlockHeight, NetworkUpgrade, Parameters};

/// Syncs a wallet to the latest state of the blockchain
pub async fn sync<P, W>(
    client: CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
    parameters: &P,
    wallet_data: &mut W,
) -> Result<(), ()>
where
    P: Parameters,
    W: SyncWallet,
{
    // create channel for sending fetch requests and launch fetcher task
    let (fetch_request_sender, fetch_request_receiver) = unbounded_channel();
    let fetcher_handle = tokio::spawn(fetcher(fetch_request_receiver, client));

    let chain_height = get_chain_height(fetch_request_sender).await.unwrap();
    update_scan_ranges(parameters, wallet_data, chain_height).unwrap();

    try_join_all(vec![fetcher_handle]).await.unwrap();

    Ok(())
}

fn update_scan_ranges<P, W>(
    parameters: &P,
    wallet_data: &mut W,
    chain_height: BlockHeight,
) -> Result<(), ()>
where
    P: Parameters,
    W: SyncWallet,
{
    let scan_ranges = wallet_data.set_sync_state().unwrap().set_scan_ranges();

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
    let chain_tip_scan_range = ScanRange::from_parts(
        Range {
            start: wallet_height,
            end: chain_height + 1,
        },
        ScanPriority::Historic,
    );
    scan_ranges.push(chain_tip_scan_range);

    // TODO: add logic to combine chain tip scan range with wallet tip scan range
    // TODO: add scan priority logic

    Ok(())
}

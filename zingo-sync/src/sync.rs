use std::ops::Range;

use crate::client::{fetcher::fetcher, get_chain_height};

use zcash_client_backend::{
    data_api::scanning::{ScanPriority, ScanRange},
    proto::service::compact_tx_streamer_client::CompactTxStreamerClient,
};

use futures::future::try_join_all;
use tokio::sync::mpsc::unbounded_channel;
use zcash_primitives::consensus::BlockHeight;

/// Syncs a wallet to the latest state of the blockchain
#[allow(dead_code)]
pub async fn sync(
    client: CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
    // TODO: add wallet data field
) -> Result<(), ()> {
    // create channel for sending fetch requests and launch fetcher task
    let (fetch_request_sender, fetch_request_receiver) = unbounded_channel();
    let fetcher_handle = tokio::spawn(fetcher(fetch_request_receiver, client));

    let chain_height = get_chain_height(fetch_request_sender).await.unwrap();
    update_scan_ranges(chain_height);

    try_join_all(vec![fetcher_handle]).await.unwrap();

    Ok(())
}

fn update_scan_ranges(chain_height: BlockHeight) {
    // TODO: load scan ranges from wallet data
    let mut scan_ranges: Vec<ScanRange> = Vec::new();

    let latest_scan_range = ScanRange::from_parts(
        Range {
            start: BlockHeight::from_u32(0), // TODO: add logic to replace with wallet height
            end: chain_height,
        },
        ScanPriority::Historic,
    );
    scan_ranges.push(latest_scan_range);

    // TODO: add logic to combine latest scan range with the tip of wallet scan ranges
}

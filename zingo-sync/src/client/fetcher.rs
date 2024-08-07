//! Queue and prioritise fetch requests to fetch data from the server

use std::ops::Range;

use tokio::sync::mpsc::UnboundedReceiver;

use zcash_client_backend::proto::{
    compact_formats::CompactBlock,
    service::{
        compact_tx_streamer_client::CompactTxStreamerClient, BlockId, BlockRange, ChainSpec,
    },
};
use zcash_primitives::consensus::BlockHeight;

use crate::client::FetchRequest;

/// Receives [`self::FetchRequest`]'s via an [`tokio::sync::mpsc::UnboundedReceiver`] for queueing,
/// prioritisation and fetching from the server.
/// Returns the data specified in the [`self::FetchRequest`] variant via the provided [`tokio::sync::mpsc::oneshot::Sender`].
///
/// Allows all requests to the server to be handled from a single task for efficiency and also enables
/// request prioritisation for further performance enhancement
pub async fn fetcher(
    mut fetch_request_receiver: UnboundedReceiver<FetchRequest>,
    mut client: CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
) -> Result<(), ()> {
    let mut fetch_request_queue: Vec<FetchRequest> = Vec::new();

    loop {
        // `fetcher` returns `Ok` here when all requests have successfully been fetched and the
        // fetch_request channel is closed on sync completion.
        if receive_fetch_requests(&mut fetch_request_receiver, &mut fetch_request_queue).await {
            return Ok(());
        }

        let fetch_request = select_fetch_request(&mut fetch_request_queue);

        if let Some(request) = fetch_request {
            fetch_from_server(&mut client, request).await.unwrap();
        }
    }
}

// receives fetch requests and populates the fetch request queue
//
// returns `true` if the fetch request channel is closed and all fetch requests have been completed,
// signalling sync is complete and no longer needs to fetch data from the server.
async fn receive_fetch_requests(
    receiver: &mut UnboundedReceiver<FetchRequest>,
    fetch_request_queue: &mut Vec<FetchRequest>,
) -> bool {
    // if there are no fetch requests to process, sleep until the next fetch request is received
    // or channel is closed
    if fetch_request_queue.is_empty() {
        while let Some(fetch_request) = receiver.recv().await {
            fetch_request_queue.push(fetch_request);
        }
    }
    // receive all remaining fetch requests from channel
    // when channel is empty return `false` to continue fetching data from the server
    // when channel is closed and all fetch requests are processed, return `true`
    loop {
        match receiver.try_recv() {
            Ok(fetch_request) => fetch_request_queue.push(fetch_request),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                if fetch_request_queue.is_empty() {
                    return true;
                } else {
                    break;
                }
            }
        }
    }

    false
}

// TODO: placeholder for algorythm that selects the next fetch request to be processed
// return `None` if a fetch request could not be selected
fn select_fetch_request(fetch_request_queue: &mut Vec<FetchRequest>) -> Option<FetchRequest> {
    // TODO: add other fetch requests with priorities
    let fetch_request_index = fetch_request_queue
        .iter()
        .enumerate()
        .find(|(_, request)| matches!(request, FetchRequest::ChainTip(_)))
        .map(|(index, _)| index);

    fetch_request_index.map(|index| fetch_request_queue.remove(index))
}

//
async fn fetch_from_server(
    client: &mut CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
    fetch_request: FetchRequest,
) -> Result<(), ()> {
    match fetch_request {
        FetchRequest::ChainTip(sender) => {
            let block_id = get_latest_block(client).await;
            sender.send(block_id).unwrap();
        }
        FetchRequest::CompactBlockRange(sender, block_range) => {
            let compact_blocks = get_block_range(client, block_range).await;
            sender.send(compact_blocks).unwrap();
        }
    }

    Ok(())
}

async fn get_latest_block(
    client: &mut CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
) -> BlockId {
    let request = tonic::Request::new(ChainSpec {});

    client.get_latest_block(request).await.unwrap().into_inner()
}

async fn get_block_range(
    client: &mut CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
    block_range: Range<BlockHeight>,
) -> Vec<CompactBlock> {
    let mut compact_blocks: Vec<CompactBlock> =
        Vec::with_capacity(u64::from(block_range.end - block_range.start) as usize);

    let request = tonic::Request::new(BlockRange {
        start: Some(BlockId {
            height: u64::from(block_range.start),
            hash: Vec::new(),
        }),
        end: Some(BlockId {
            height: u64::from(block_range.end) - 1,
            hash: Vec::new(),
        }),
    });
    let mut block_stream = client.get_block_range(request).await.unwrap().into_inner();

    while let Some(compact_block) = block_stream.message().await.unwrap() {
        compact_blocks.push(compact_block);
    }

    compact_blocks
}

//! Queue and prioritise fetch requests to fetch data from the server

use std::ops::Range;

use tokio::sync::mpsc::UnboundedReceiver;

use zcash_client_backend::proto::{
    compact_formats::CompactBlock,
    service::{
        compact_tx_streamer_client::CompactTxStreamerClient, BlockId, BlockRange, ChainSpec,
        TreeState, TxFilter,
    },
};
use zcash_primitives::{
    consensus::{self, BlockHeight, BranchId},
    transaction::{Transaction, TxId},
};

use crate::client::FetchRequest;

/// Receives [`self::FetchRequest`]'s via an [`tokio::sync::mpsc::UnboundedReceiver`] for queueing,
/// prioritisation and fetching from the server.
/// Returns the data specified in the [`self::FetchRequest`] variant via the provided [`tokio::sync::oneshot::Sender`].
///
/// Allows all requests to the server to be handled from a single task for efficiency and also enables
/// request prioritisation for further performance enhancement
pub async fn fetch(
    mut fetch_request_receiver: UnboundedReceiver<FetchRequest>,
    mut client: CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
    consensus_parameters: impl consensus::Parameters,
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
            fetch_from_server(&mut client, &consensus_parameters, request)
                .await
                .unwrap();
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
        if let Some(fetch_request) = receiver.recv().await {
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
    // TODO: improve priority logic
    if fetch_request_queue.first().is_some() {
        Some(fetch_request_queue.remove(0))
    } else {
        None
    }
}

//
async fn fetch_from_server(
    client: &mut CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
    parameters: &impl consensus::Parameters,
    fetch_request: FetchRequest,
) -> Result<(), ()> {
    match fetch_request {
        FetchRequest::ChainTip(sender) => {
            tracing::info!("Fetching chain tip.");
            let block_id = get_latest_block(client).await.unwrap();
            sender.send(block_id).unwrap();
        }
        FetchRequest::CompactBlockRange(sender, block_range) => {
            tracing::info!("Fetching compact blocks. {:?}", &block_range);
            let compact_blocks = get_block_range(client, block_range).await.unwrap();
            sender.send(compact_blocks).unwrap();
        }
        FetchRequest::TreeState(sender, block_height) => {
            tracing::info!("Fetching tree state. {:?}", &block_height);
            let tree_state = get_tree_state(client, block_height).await.unwrap();
            sender.send(tree_state).unwrap();
        }
        FetchRequest::Transaction(sender, txid) => {
            tracing::info!("Fetching transaction. {:?}", txid);
            let transaction = get_transaction(client, parameters, txid).await.unwrap();
            sender.send(transaction).unwrap();
        }
    }

    Ok(())
}

async fn get_latest_block(
    client: &mut CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
) -> Result<BlockId, ()> {
    let request = tonic::Request::new(ChainSpec {});

    Ok(client.get_latest_block(request).await.unwrap().into_inner())
}
async fn get_block_range(
    client: &mut CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
    block_range: Range<BlockHeight>,
) -> Result<Vec<CompactBlock>, ()> {
    let mut compact_blocks: Vec<CompactBlock> =
        Vec::with_capacity(u64::from(block_range.end - block_range.start) as usize);

    let request = tonic::Request::new(BlockRange {
        start: Some(BlockId {
            height: u64::from(block_range.start),
            hash: vec![],
        }),
        end: Some(BlockId {
            height: u64::from(block_range.end) - 1,
            hash: vec![],
        }),
    });
    let mut block_stream = client.get_block_range(request).await.unwrap().into_inner();

    while let Some(compact_block) = block_stream.message().await.unwrap() {
        compact_blocks.push(compact_block);
    }

    Ok(compact_blocks)
}
async fn get_tree_state(
    client: &mut CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
    block_height: BlockHeight,
) -> Result<TreeState, ()> {
    let request = tonic::Request::new(BlockId {
        height: block_height.into(),
        hash: vec![],
    });

    Ok(client.get_tree_state(request).await.unwrap().into_inner())
}

async fn get_transaction(
    client: &mut CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
    parameters: &impl consensus::Parameters,
    txid: TxId,
) -> Result<(Transaction, BlockHeight), ()> {
    let request = tonic::Request::new(TxFilter {
        block: None,
        index: 0,
        hash: txid.as_ref().to_vec(),
    });

    let raw_transaction = client.get_transaction(request).await.unwrap().into_inner();
    let block_height = BlockHeight::from_u32(raw_transaction.height as u32);

    let transaction = Transaction::read(
        &raw_transaction.data[..],
        BranchId::for_height(parameters, block_height),
    )
    .unwrap();

    Ok((transaction, block_height))
}

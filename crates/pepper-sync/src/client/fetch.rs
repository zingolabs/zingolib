//! Queue and prioritise fetch requests to fetch data from the server

use tokio::sync::mpsc::UnboundedReceiver;

use tracing::instrument;
use zcash_protocol::consensus::BlockHeight;
use zl_netutils::{Indexer, TransparentIndexer};

use crate::{client::FetchRequest, protocol};

use zl_netutils::time::{FINITE_STREAM_TIMEOUT, UNARY_RPC_TIMEOUT};

/// Receives [`self::FetchRequest`]'s via an [`tokio::sync::mpsc::UnboundedReceiver`] for queueing,
/// prioritisation and fetching from the server.
/// Returns the data specified in the [`self::FetchRequest`] variant via the provided [`tokio::sync::oneshot::Sender`].
///
/// Allows all requests to the server to be handled from a single task for efficiency and also enables
/// request prioritisation for further performance enhancement
pub(crate) async fn fetch<C>(
    mut fetch_request_receiver: UnboundedReceiver<FetchRequest>,
    mut client: C,
) where
    C: Clone + Indexer + TransparentIndexer + Sync + Send + 'static,
{
    let mut fetch_request_queue: Vec<FetchRequest> = Vec::new();

    loop {
        // `fetcher` returns `Ok` here when all requests have successfully been fetched and the
        // fetch_request channel is closed on sync completion.
        if receive_fetch_requests(&mut fetch_request_receiver, &mut fetch_request_queue).await {
            return;
        }

        let fetch_request = select_fetch_request(&mut fetch_request_queue);

        if let Some(request) = fetch_request {
            fetch_from_server(&mut client, request).await;
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
    if fetch_request_queue.is_empty()
        && let Some(fetch_request) = receiver.recv().await
    {
        fetch_request_queue.push(fetch_request);
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
                }
                break;
            }
        }
    }

    false
}

// TODO: placeholder for algorithm that selects the next fetch request to be processed
// return `None` if a fetch request could not be selected
fn select_fetch_request(fetch_request_queue: &mut Vec<FetchRequest>) -> Option<FetchRequest> {
    // TODO: improve priority logic
    if fetch_request_queue.is_empty() {
        None
    } else {
        Some(fetch_request_queue.remove(0))
    }
}

//
async fn fetch_from_server<C>(client: &mut C, fetch_request: FetchRequest)
where
    C: Clone + Indexer + TransparentIndexer + Sync + Send + 'static,
{
    match fetch_request {
        FetchRequest::ChainTip(sender) => {
            tracing::debug!("Fetching chain tip.");
            let block_id = get_latest_block(client).await;
            let _ignore_error = sender.send(block_id);
        }
        FetchRequest::CompactBlock(sender, block_height) => {
            tracing::debug!("Fetching compact block. {:?}", &block_height);
            let block = client
                .get_block(protocol::block_id_at(block_height), UNARY_RPC_TIMEOUT)
                .await;
            let _ignore_error = sender.send(block);
        }
        FetchRequest::CompactBlockRange(sender, block_range, include_transparent) => {
            tracing::debug!(
                "Fetching compact blocks. {:?}. include transparent: {}",
                &block_range,
                include_transparent
            );
            let pool_types = protocol::pool_types(include_transparent);
            let block_stream = match protocol::block_range(block_range, pool_types) {
                Ok(range) => client.get_block_range(range, FINITE_STREAM_TIMEOUT).await,
                Err(refusal) => Err(refusal),
            };
            let _ignore_error = sender.send(block_stream);
        }
        FetchRequest::NullifierRange(sender, block_range) => {
            tracing::debug!("Fetching nullifiers. {:?}", &block_range);
            #[allow(deprecated)]
            let block_stream = match protocol::nullifier_range(block_range) {
                Ok(range) => {
                    client
                        .get_block_range_nullifiers(range, FINITE_STREAM_TIMEOUT)
                        .await
                }
                Err(refusal) => Err(refusal),
            };
            let _ignore_error = sender.send(block_stream);
        }
        FetchRequest::SubtreeRoots(sender, start_index, shielded_protocol, max_entries) => {
            tracing::debug!(
                "Fetching subtree roots. start index: {}. shielded protocol: {}",
                start_index,
                shielded_protocol
            );
            let subtree_roots = client
                .get_subtree_roots(
                    protocol::subtree_roots_arg(start_index, shielded_protocol, max_entries),
                    FINITE_STREAM_TIMEOUT,
                )
                .await;
            let _ignore_error = sender.send(subtree_roots);
        }
        FetchRequest::TreeState(sender, block_height) => {
            tracing::debug!("Fetching tree state. {:?}", &block_height);
            let tree_state = get_tree_state(client, block_height).await;
            let _ignore_error = sender.send(tree_state);
        }
        FetchRequest::Transaction(sender, txid) => {
            tracing::debug!("Fetching transaction. {:?}", txid);
            let transaction = client
                .get_transaction(protocol::tx_filter(txid), UNARY_RPC_TIMEOUT)
                .await;
            let _ignore_error = sender.send(transaction);
        }
        FetchRequest::UtxoMetadata(sender, (addresses, start_height)) => {
            tracing::debug!(
                "Fetching unspent transparent output metadata from {:?} for addresses:\n{:?}",
                &start_height,
                &addresses
            );
            let utxo_metadata = client
                .get_address_utxos(
                    protocol::address_utxos_arg(
                        addresses,
                        start_height,
                        protocol::UNLIMITED_ENTRIES,
                    ),
                    UNARY_RPC_TIMEOUT,
                )
                .await
                .map(|reply| reply.address_utxos);
            let _ignore_error = sender.send(utxo_metadata);
        }
        FetchRequest::TransparentAddressTxs(sender, (address, block_range)) => {
            tracing::debug!(
                "Fetching raw transactions in block range {:?} for address {:?}",
                &block_range,
                &address
            );
            #[allow(deprecated)]
            let raw_transaction_stream = match protocol::address_block_filter(address, block_range)
            {
                Ok(filter) => {
                    client
                        .get_taddress_txids(filter, FINITE_STREAM_TIMEOUT)
                        .await
                }
                Err(refusal) => Err(refusal),
            };
            let _ignore_error = sender.send(raw_transaction_stream);
        }
    }
}

#[instrument(skip(client), name = "fetch::get_lightd_info", err, level = "info")]
pub(super) async fn get_lightd_info<C>(
    client: &mut C,
) -> Result<protocol::LightdInfo, protocol::Status>
where
    C: Indexer,
{
    client.get_lightd_info(UNARY_RPC_TIMEOUT).await
}

#[instrument(skip(client), name = "fetch::get_latest_block", err, level = "info")]
async fn get_latest_block<C>(client: &mut C) -> Result<protocol::BlockId, protocol::Status>
where
    C: Clone + Indexer + TransparentIndexer + Sync + Send + 'static,
{
    client.get_latest_block(UNARY_RPC_TIMEOUT).await
}

#[instrument(skip(client), name = "fetch::get_tree_state", err, level = "info")]
async fn get_tree_state<C>(
    client: &mut C,
    block_height: BlockHeight,
) -> Result<protocol::TreeState, protocol::Status>
where
    C: Clone + Indexer + TransparentIndexer + Sync + Send + 'static,
{
    client
        .get_tree_state(protocol::block_id_at(block_height), UNARY_RPC_TIMEOUT)
        .await
}

/// Call `GetMempoolStream` client gPRC.
///
/// This is not called from the fetch request framework and is intended to be called independently.
pub(crate) async fn get_mempool_stream<C>(
    client: &mut C,
) -> Result<protocol::RawTransactions, protocol::Status>
where
    C: Clone + Indexer + TransparentIndexer + Sync + Send + 'static,
{
    client.get_mempool_stream().await
}

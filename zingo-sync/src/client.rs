//! Module for handling all connections to the server

use std::ops::Range;

use tokio::sync::{mpsc::UnboundedSender, oneshot};

use zcash_client_backend::{
    data_api::chain::ChainState,
    proto::{
        compact_formats::CompactBlock,
        service::{
            compact_tx_streamer_client::CompactTxStreamerClient, BlockId, GetAddressUtxosReply,
            RawTransaction, SubtreeRoot, TreeState,
        },
    },
};
use zcash_primitives::{
    consensus::BlockHeight,
    transaction::{Transaction, TxId},
};

pub mod fetch;

/// Fetch requests are created and sent to the [`crate::client::fetch::fetch`] task when a connection to the server is required.
///
/// Each variant includes a [`tokio::sync::oneshot::Sender`] for returning the fetched data to the requester.
#[derive(Debug)]
pub enum FetchRequest {
    /// Gets the height of the blockchain from the server.
    ChainTip(oneshot::Sender<BlockId>),
    /// Gets the specified range of compact blocks from the server (end exclusive).
    CompactBlockRange(oneshot::Sender<Vec<CompactBlock>>, Range<BlockHeight>),
    /// Gets the tree states for a specified block height.
    TreeState(oneshot::Sender<TreeState>, BlockHeight),
    /// Get a full transaction by txid.
    Transaction(oneshot::Sender<(Transaction, BlockHeight)>, TxId),
    /// Get a list of unspent transparent output metadata for a given list of transparent addresses and start height.
    UtxoMetadata(
        oneshot::Sender<Vec<GetAddressUtxosReply>>,
        (Vec<String>, BlockHeight),
    ),
    /// Get a list of transactions for a given transparent address and block range.
    TransparentAddressTxs(
        oneshot::Sender<Vec<(BlockHeight, Transaction)>>,
        (String, Range<BlockHeight>),
    ),
    /// Get a stream of shards.
    GetSubtreeRoots(
        oneshot::Sender<tonic::Streaming<SubtreeRoot>>,
        u32,
        i32,
        u32,
    ),
}

/// Gets the height of the blockchain from the server.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub async fn get_chain_height(
    fetch_request_sender: UnboundedSender<FetchRequest>,
) -> Result<BlockHeight, ()> {
    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::ChainTip(reply_sender))
        .unwrap();
    let chain_tip = reply_receiver.await.unwrap();

    Ok(BlockHeight::from_u32(chain_tip.height as u32))
}

/// Gets the specified range of compact blocks from the server (end exclusive).
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub async fn get_compact_block_range(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    block_range: Range<BlockHeight>,
) -> Result<Vec<CompactBlock>, ()> {
    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::CompactBlockRange(reply_sender, block_range))
        .unwrap();
    let compact_blocks = reply_receiver.await.unwrap();

    Ok(compact_blocks)
}

/// Gets the stream of shards (subtree roots)
/// from the server.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub async fn get_subtree_roots(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    start_index: u32,
    shielded_protocol: i32,
    max_entries: u32,
) -> Result<tonic::Streaming<SubtreeRoot>, ()> {
    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::GetSubtreeRoots(
            reply_sender,
            start_index,
            shielded_protocol,
            max_entries,
        ))
        .unwrap();
    let shards = reply_receiver.await.unwrap();
    Ok(shards)
}

/// Gets the frontiers for a specified block height.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub async fn get_frontiers(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    block_height: BlockHeight,
) -> Result<ChainState, ()> {
    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::TreeState(reply_sender, block_height))
        .unwrap();
    let tree_state = reply_receiver.await.unwrap();
    let frontiers = tree_state.to_chain_state().unwrap();

    Ok(frontiers)
}

/// Gets a full transaction for a specified txid.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub async fn get_transaction_and_block_height(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    txid: TxId,
) -> Result<(Transaction, BlockHeight), ()> {
    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::Transaction(reply_sender, txid))
        .unwrap();
    let transaction_and_block_height = reply_receiver.await.unwrap();

    Ok(transaction_and_block_height)
}

/// Gets unspent transparent output metadata for a list of `transparent addresses` from the specified `start_height`.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub async fn get_utxo_metadata(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    transparent_addresses: Vec<String>,
    start_height: BlockHeight,
) -> Result<Vec<GetAddressUtxosReply>, ()> {
    if transparent_addresses.is_empty() {
        panic!("addresses must be non-empty!");
    }

    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::UtxoMetadata(
            reply_sender,
            (transparent_addresses, start_height),
        ))
        .unwrap();
    let transparent_output_metadata = reply_receiver.await.unwrap();

    Ok(transparent_output_metadata)
}

/// Gets transactions relevant to a given `transparent address` in the specified `block_range`.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub async fn get_transparent_address_transactions(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    transparent_address: String,
    block_range: Range<BlockHeight>,
) -> Result<Vec<(BlockHeight, Transaction)>, ()> {
    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::TransparentAddressTxs(
            reply_sender,
            (transparent_address, block_range),
        ))
        .unwrap();
    let transactions = reply_receiver.await.unwrap();

    Ok(transactions)
}

/// Gets stream of mempool transactions until the next block is mined.
pub async fn get_mempool_transaction_stream(
    client: &mut CompactTxStreamerClient<zingo_netutils::UnderlyingService>,
) -> Result<tonic::Streaming<RawTransaction>, ()> {
    tracing::info!("Fetching mempool stream");
    let mempool_stream = fetch::get_mempool_stream(client).await.unwrap();

    Ok(mempool_stream)
}

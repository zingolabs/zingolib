//! Module for handling all connections to the server

use std::ops::Range;

use zcash_client_backend::proto::{compact_formats::CompactBlock, service::BlockId};
use zcash_primitives::consensus::BlockHeight;

use tokio::sync::{mpsc::UnboundedSender, oneshot};

pub mod fetcher;

/// Fetch requests are created and sent to the [`crate::client::fetcher::fetcher`] task when a connection to the server is required.
///
/// Each variant includes a [`tokio::sync::mpsc::oneshot::Sender`] for returning the fetched data to the requester.
pub enum FetchRequest {
    ChainTip(oneshot::Sender<BlockId>),
    CompactBlockRange(oneshot::Sender<Vec<CompactBlock>>, Range<BlockHeight>),
}

/// Gets the height of the blockchain from the server.
///
/// Requires [`crate::client::fetcher::fetcher`] to be running concurrently, connected via the `fetch_request` channel.
pub async fn get_chain_height(
    fetch_request_sender: UnboundedSender<FetchRequest>,
) -> Result<BlockHeight, ()> {
    let (sender, receiver) = oneshot::channel::<BlockId>();
    fetch_request_sender
        .send(FetchRequest::ChainTip(sender))
        .unwrap();
    let chain_tip = receiver.await.unwrap();

    Ok(BlockHeight::from_u32(chain_tip.height as u32))
}
/// Gets the height of the blockchain from the server.
///
/// Requires [`crate::client::fetcher::fetcher`] to be running concurrently, connected via the `fetch_request` channel.
#[allow(dead_code)]
pub async fn get_compact_block_range(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    block_range: Range<BlockHeight>,
) -> Result<Vec<CompactBlock>, ()> {
    let (sender, receiver) = oneshot::channel::<Vec<CompactBlock>>();
    fetch_request_sender
        .send(FetchRequest::CompactBlockRange(sender, block_range))
        .unwrap();
    let compact_blocks = receiver.await.unwrap();

    Ok(compact_blocks)
}

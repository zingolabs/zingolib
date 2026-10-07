//! Module for handling all connections to the server

use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{self, AtomicBool},
    },
    time::Duration,
};

use tokio::sync::{mpsc::UnboundedSender, oneshot};

use zcash_primitives::transaction::{Transaction, TxId};
use zcash_protocol::{
    PoolType, ShieldedPool,
    consensus::{self, BlockHeight},
};

use zingo_netutils::{
    Indexer, TransparentIndexer,
    lightwallet_protocol::{
        BlockId, CompactBlock, GetAddressUtxosReply, RawTransaction, TreeState,
    },
};

use crate::{
    error::{MempoolError, ServerError},
    witness::Frontiers,
};

use zingo_netutils::lightwallet_protocol::SubtreeRoot;

pub(crate) mod fetch;

const MAX_RETRIES: u8 = 3;

use zingo_netutils::time::STREAM_MSG_TIMEOUT;

async fn next_stream_item<T>(
    stream: &mut tonic::Streaming<T>,
    what: &'static str,
) -> Result<Option<T>, tonic::Status> {
    match tokio::time::timeout(STREAM_MSG_TIMEOUT, stream.message()).await {
        Ok(res) => res,
        Err(_) => Err(tonic::Status::deadline_exceeded(format!(
            "{what} stream message timeout"
        ))),
    }
}

/// Fetch requests are created and sent to the [`crate::client::fetch::fetch`] task when a connection to the server is required.
///
/// Each variant includes a [`tokio::sync::oneshot::Sender`] for returning the fetched data to the requester.
#[derive(Debug)]
pub enum FetchRequest {
    /// Gets the height of the blockchain from the server.
    ChainTip(oneshot::Sender<Result<BlockId, tonic::Status>>),
    /// Gets  a compact block of the given block height.
    CompactBlock(
        oneshot::Sender<Result<CompactBlock, tonic::Status>>,
        BlockHeight,
    ),
    /// Gets the specified range of compact blocks from the server (end exclusive).
    ///
    /// Compact blocks include transparent data if the `bool` is true, otherwise only shielded data.
    CompactBlockRange(
        oneshot::Sender<Result<tonic::Streaming<CompactBlock>, tonic::Status>>,
        Range<BlockHeight>,
        bool,
    ),
    /// Gets the specified range of nullifiers from the server (end exclusive).
    NullifierRange(
        oneshot::Sender<Result<tonic::Streaming<CompactBlock>, tonic::Status>>,
        Range<BlockHeight>,
    ),
    /// Gets the tree states for a specified block height.
    TreeState(
        oneshot::Sender<Result<TreeState, tonic::Status>>,
        BlockHeight,
    ),
    /// Get a full transaction by txid.
    Transaction(oneshot::Sender<Result<RawTransaction, tonic::Status>>, TxId),
    /// Get a list of unspent transparent output metadata for a given list of transparent addresses and start height.
    #[allow(dead_code)]
    UtxoMetadata(
        oneshot::Sender<Result<Vec<GetAddressUtxosReply>, tonic::Status>>,
        (Vec<String>, BlockHeight),
    ),
    /// Get a list of transactions for a given transparent address and block range.
    TransparentAddressTxs(
        oneshot::Sender<Result<tonic::Streaming<RawTransaction>, tonic::Status>>,
        (String, Range<BlockHeight>),
    ),
    /// Get a stream of shards.
    SubtreeRoots(
        oneshot::Sender<Result<tonic::Streaming<SubtreeRoot>, tonic::Status>>,
        u32,
        i32,
        u32,
    ),
}

/// Minimum lightwallet protocol version the server must serve. v0.4.0 added transparent data to compact blocks and
/// v0.5.0 added the Ironwood pool.
const MIN_LIGHTWALLET_PROTOCOL_VERSION: (u64, u64, u64) = (0, 5, 0);

/// Checks the server's lightwallet protocol version is at least [`MIN_LIGHTWALLET_PROTOCOL_VERSION`] so that it serves
/// the transparent and Ironwood data in compact blocks required for sync.
///
/// Servers that pre-date the protocol version field do not set it and are rejected.
pub(crate) async fn check_lightwallet_protocol_version<C>(client: &mut C) -> Result<(), ServerError>
where
    C: Indexer,
{
    let version = fetch::get_lightd_info(client)
        .await?
        .lightwallet_protocol_version;

    if parse_protocol_version(&version)
        .is_some_and(|version| version >= MIN_LIGHTWALLET_PROTOCOL_VERSION)
    {
        Ok(())
    } else {
        Err(ServerError::UnsupportedProtocolVersion { version })
    }
}

/// Parses a `major.minor.patch` version with an optional `v` prefix. Any pre-release or build suffix on the patch
/// version is ignored.
fn parse_protocol_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim().trim_start_matches('v').splitn(3, '.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?;
    let patch = patch[..patch
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(patch.len())]
        .parse()
        .ok()?;

    Some((major, minor, patch))
}

/// Gets the height of the blockchain from the server.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub(crate) async fn get_chain_height(
    fetch_request_sender: UnboundedSender<FetchRequest>,
) -> Result<BlockHeight, ServerError> {
    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::ChainTip(reply_sender))
        .map_err(|_| ServerError::FetcherDropped)?;

    let chain_tip = reply_receiver
        .await
        .map_err(|_| ServerError::FetcherDropped)?
        .map_err(ServerError::RequestFailed)?;

    Ok(BlockHeight::from_u32(chain_tip.height as u32))
}

/// Gets the specified range of compact blocks from the server (end exclusive).
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub(crate) async fn get_compact_block(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    block_height: BlockHeight,
) -> Result<CompactBlock, ServerError> {
    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::CompactBlock(reply_sender, block_height))
        .map_err(|_| ServerError::FetcherDropped)?;

    let block = reply_receiver
        .await
        .map_err(|_| ServerError::FetcherDropped)?
        .map_err(ServerError::RequestFailed)?;

    Ok(block)
}

/// Gets the specified range of compact blocks from the server (end exclusive).
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub(crate) async fn get_compact_block_range(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    block_range: Range<BlockHeight>,
    include_transparent: bool,
) -> Result<tonic::Streaming<CompactBlock>, ServerError> {
    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::CompactBlockRange(
            reply_sender,
            block_range,
            include_transparent,
        ))
        .map_err(|_| ServerError::FetcherDropped)?;

    let block_stream = reply_receiver
        .await
        .map_err(|_| ServerError::FetcherDropped)?
        .map_err(ServerError::RequestFailed)?;

    Ok(block_stream)
}

/// Gets the specified range of nullifiers from the server (end exclusive).
///
/// Nullifiers are stored in compact blocks where the actions contain only nullifiers.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub(crate) async fn get_nullifier_range(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    block_range: Range<BlockHeight>,
) -> Result<tonic::Streaming<CompactBlock>, ServerError> {
    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::NullifierRange(reply_sender, block_range))
        .map_err(|_| ServerError::FetcherDropped)?;

    let block_stream = reply_receiver
        .await
        .map_err(|_| ServerError::FetcherDropped)?
        .map_err(ServerError::RequestFailed)?;

    Ok(block_stream)
}

/// Gets the stream of shards (subtree roots)
/// from the server.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub(crate) async fn get_subtree_roots(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    mut start_index: u32,
    shielded_protocol: i32,
    max_entries: u32,
) -> Result<Vec<SubtreeRoot>, ServerError> {
    let mut subtree_roots = Vec::new();
    let mut retry_count = 0;

    'retry: loop {
        let roots_before_pass = subtree_roots.len();
        let (reply_sender, reply_receiver) = oneshot::channel();

        fetch_request_sender
            .send(FetchRequest::SubtreeRoots(
                reply_sender,
                start_index,
                shielded_protocol,
                max_entries,
            ))
            .map_err(|_| ServerError::FetcherDropped)?;

        let mut subtree_root_stream = reply_receiver
            .await
            .map_err(|_| ServerError::FetcherDropped)?
            .map_err(ServerError::RequestFailed)?;

        while let Some(subtree_root) =
            match next_stream_item(&mut subtree_root_stream, "SubtreeRoots").await {
                Ok(s) => s,
                Err(e)
                    if (e.code() == tonic::Code::DeadlineExceeded
                        || e.message().contains("Unexpected EOF decoding stream."))
                        && retry_count < MAX_RETRIES =>
                {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    retry_count += 1;
                    continue 'retry;
                }
                Err(e) => return Err(e.into()),
            }
        {
            subtree_roots.push(subtree_root);
            start_index += 1;
        }

        // For an unbounded request, a clean stream end is only trusted once
        // a resume pass from the current index comes back empty: a stream
        // cut mid-flight (proxy, flow control) also ends cleanly, and
        // accepting it here silently truncates the wallet's shard tree. The
        // confirmation costs one extra empty round-trip on the happy path.
        // A bounded request (max_entries != 0) keeps single-pass semantics,
        // because resuming would fetch past the caller's cap.
        if max_entries != 0 || subtree_roots.len() == roots_before_pass {
            break 'retry;
        }
    }

    Ok(subtree_roots)
}

/// Gets the frontiers for a specified block height.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub(crate) async fn get_frontiers(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    consensus_parameters: &impl consensus::Parameters,
    block_height: BlockHeight,
) -> Result<Frontiers, ServerError> {
    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::TreeState(reply_sender, block_height))
        .map_err(|_| ServerError::FetcherDropped)?;

    let tree_state = reply_receiver
        .await
        .map_err(|_| ServerError::FetcherDropped)?
        .map_err(ServerError::RequestFailed)?;

    // servers omit a pool's tree state below the pool's activation height. at and above the activation height, an
    // empty tree is served as a serialized empty tree, so an omitted tree state means the server does not serve the
    // pool.
    for (pool, tree, network_upgrade) in [
        (
            ShieldedPool::Sapling,
            &tree_state.sapling_tree,
            consensus::NetworkUpgrade::Sapling,
        ),
        (
            ShieldedPool::Orchard,
            &tree_state.orchard_tree,
            consensus::NetworkUpgrade::Nu5,
        ),
        (
            ShieldedPool::Ironwood,
            &tree_state.ironwood_tree,
            consensus::NetworkUpgrade::Nu6_3,
        ),
    ] {
        if tree.is_empty() && consensus_parameters.is_nu_active(network_upgrade, block_height) {
            return Err(ServerError::TreeStateNotServed {
                pool: PoolType::Shielded(pool),
                height: block_height,
            });
        }
    }

    tree_state.try_into().map_err(ServerError::InvalidFrontier)
}

/// Gets a full transaction for a specified txid.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub(crate) async fn get_transaction_and_block_height(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    consensus_parameters: &impl consensus::Parameters,
    txid: TxId,
) -> Result<(Transaction, BlockHeight), ServerError> {
    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::Transaction(reply_sender, txid))
        .map_err(|_| ServerError::FetcherDropped)?;

    let raw_transaction = reply_receiver
        .await
        .map_err(|_| ServerError::FetcherDropped)?
        .map_err(ServerError::RequestFailed)?;
    let block_height =
        BlockHeight::from_u32(u32::try_from(raw_transaction.height).expect("should be valid u32"));
    let transaction = Transaction::read(
        &raw_transaction.data[..],
        consensus::BranchId::for_height(consensus_parameters, block_height),
    )
    .map_err(ServerError::InvalidTransaction)?;

    Ok((transaction, block_height))
}

/// Gets unspent transparent output metadata for a list of `transparent addresses` from the specified `start_height`.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
#[allow(dead_code)]
pub(crate) async fn get_utxo_metadata(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    transparent_addresses: Vec<String>,
    start_height: BlockHeight,
) -> Result<Vec<GetAddressUtxosReply>, ServerError> {
    if transparent_addresses.is_empty() {
        return Ok(Vec::new());
    }

    let (reply_sender, reply_receiver) = oneshot::channel();

    fetch_request_sender
        .send(FetchRequest::UtxoMetadata(
            reply_sender,
            (transparent_addresses, start_height),
        ))
        .map_err(|_| ServerError::FetcherDropped)?;

    reply_receiver
        .await
        .map_err(|_| ServerError::FetcherDropped)?
        .map_err(ServerError::RequestFailed)
}

/// Gets transactions relevant to a given `transparent address` in the specified `block_range`.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub(crate) async fn get_transparent_address_transactions(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    consensus_parameters: &impl consensus::Parameters,
    transparent_address: String,
    block_range: Range<BlockHeight>,
) -> Result<Vec<(BlockHeight, Transaction)>, ServerError> {
    let mut raw_transactions: Vec<RawTransaction> = Vec::new();
    let mut retry_count = 0;

    'retry: loop {
        let (reply_sender, reply_receiver) = oneshot::channel();

        fetch_request_sender
            .send(FetchRequest::TransparentAddressTxs(
                reply_sender,
                (transparent_address.clone(), block_range.clone()),
            ))
            .map_err(|_| ServerError::FetcherDropped)?;

        let mut raw_transaction_stream = reply_receiver
            .await
            .map_err(|_| ServerError::FetcherDropped)?
            .map_err(ServerError::RequestFailed)?;

        while let Some(raw_tx) =
            match next_stream_item(&mut raw_transaction_stream, "TransparentAddressTxs").await {
                Ok(s) => s,
                Err(e)
                    if (e.code() == tonic::Code::DeadlineExceeded
                        || e.message().contains("Unexpected EOF decoding stream."))
                        && retry_count < MAX_RETRIES =>
                {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    retry_count += 1;
                    raw_transactions.clear();
                    continue 'retry;
                }
                Err(e) => return Err(e.into()),
            }
        {
            raw_transactions.push(raw_tx);
        }

        break 'retry;
    }

    let transactions = raw_transactions
        .into_iter()
        .map(|raw_transaction| {
            let block_height = BlockHeight::from_u32(
                u32::try_from(raw_transaction.height).expect("should be valid u32"),
            );

            let transaction = Transaction::read(
                &raw_transaction.data[..],
                consensus::BranchId::for_height(consensus_parameters, block_height),
            )
            .map_err(ServerError::InvalidTransaction)?;

            Ok((block_height, transaction))
        })
        .collect::<Result<Vec<(BlockHeight, Transaction)>, ServerError>>()?;

    Ok(transactions)
}

/// Gets stream of mempool transactions until the next block is mined.
///
/// Checks at intervals if `shutdown_mempool` is set to prevent hanging on awating mempool monitor handle.
pub(crate) async fn get_mempool_transaction_stream<C>(
    client: &mut C,
    shutdown_mempool: Arc<AtomicBool>,
) -> Result<tonic::Streaming<RawTransaction>, MempoolError>
where
    C: Clone + Indexer + TransparentIndexer + Sync + Send + 'static,
{
    tracing::debug!("Fetching mempool stream");
    let mut interval = tokio::time::interval(Duration::from_secs(3));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    interval.tick().await;
    loop {
        tokio::select! {
            mempool_stream_response = fetch::get_mempool_stream(client) => {
                return mempool_stream_response.map_err(|e| MempoolError::ServerError(ServerError::RequestFailed(e)));
            }

            _ = interval.tick() => {
                if shutdown_mempool.load(atomic::Ordering::Acquire) {
                    return Err(MempoolError::ShutdownWithoutStream);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;
    use zcash_protocol::local_consensus::LocalNetwork;
    use zingo_netutils::lightwallet_protocol::TreeState;

    use super::*;

    const SAPLING_ACTIVATION: u32 = 100;
    const ORCHARD_ACTIVATION: u32 = 200;
    const IRONWOOD_ACTIVATION: u32 = 300;

    const NETWORK: LocalNetwork = LocalNetwork {
        overwinter: Some(BlockHeight::from_u32(1)),
        sapling: Some(BlockHeight::from_u32(SAPLING_ACTIVATION)),
        blossom: Some(BlockHeight::from_u32(SAPLING_ACTIVATION)),
        heartwood: Some(BlockHeight::from_u32(SAPLING_ACTIVATION)),
        canopy: Some(BlockHeight::from_u32(SAPLING_ACTIVATION)),
        nu5: Some(BlockHeight::from_u32(ORCHARD_ACTIVATION)),
        nu6: Some(BlockHeight::from_u32(ORCHARD_ACTIVATION)),
        nu6_1: Some(BlockHeight::from_u32(ORCHARD_ACTIVATION)),
        nu6_2: Some(BlockHeight::from_u32(ORCHARD_ACTIVATION)),
        nu6_3: Some(BlockHeight::from_u32(IRONWOOD_ACTIVATION)),
        nu7: None,
    };

    /// Serialized empty commitment tree, as served at and above a pool's activation height.
    const EMPTY_TREE: &str = "000000";

    /// Answers tree state requests with an empty tree for every pool except `omitted_pool`, whose tree state field
    /// is left empty.
    fn spawn_fetcher(omitted_pool: ShieldedPool) -> mpsc::UnboundedSender<FetchRequest> {
        let tree = |pool: ShieldedPool| {
            if pool == omitted_pool {
                String::new()
            } else {
                EMPTY_TREE.to_string()
            }
        };
        let (sapling_tree, orchard_tree, ironwood_tree) = (
            tree(ShieldedPool::Sapling),
            tree(ShieldedPool::Orchard),
            tree(ShieldedPool::Ironwood),
        );
        let (fetch_request_sender, mut fetch_request_receiver) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(fetch_request) = fetch_request_receiver.recv().await {
                match fetch_request {
                    FetchRequest::TreeState(reply_sender, block_height) => {
                        let _ignore_error = reply_sender.send(Ok(TreeState {
                            height: u64::from(block_height),
                            hash: "00".repeat(32),
                            sapling_tree: sapling_tree.clone(),
                            orchard_tree: orchard_tree.clone(),
                            ironwood_tree: ironwood_tree.clone(),
                            ..Default::default()
                        }));
                    }
                    _ => panic!("unexpected fetch request"),
                }
            }
        });

        fetch_request_sender
    }

    /// Servers omit a pool's tree state below the pool's activation height and serve an empty tree as `000000` at
    /// the activation height, so an omitted tree state at or above the activation height means the server does not
    /// serve the pool.
    #[tokio::test]
    async fn omitted_tree_state_is_rejected_at_or_above_activation() {
        for (pool, activation_height) in [
            (ShieldedPool::Sapling, SAPLING_ACTIVATION),
            (ShieldedPool::Orchard, ORCHARD_ACTIVATION),
            (ShieldedPool::Ironwood, IRONWOOD_ACTIVATION),
        ] {
            get_frontiers(
                spawn_fetcher(pool),
                &NETWORK,
                (activation_height - 1).into(),
            )
            .await
            .unwrap();

            for height in [activation_height, activation_height + 1] {
                assert!(matches!(
                    get_frontiers(spawn_fetcher(pool), &NETWORK, height.into()).await,
                    Err(ServerError::TreeStateNotServed { pool: PoolType::Shielded(p), height: h })
                        if p == pool && h == height.into()
                ));
            }
        }
    }

    /// Zaino reports the protocol version with a `v` prefix. Servers that pre-date the field report an empty string.
    #[test]
    fn protocol_version_is_parsed_and_compared() {
        for (version, supported) in [
            ("v0.5.0", true),
            ("0.5.0", true),
            ("v0.5.1-rc.1", true),
            ("v1.0.0", true),
            ("v0.4.1", false),
            ("v0.4.0", false),
            ("", false),
            ("v0.5", false),
            ("unknown", false),
        ] {
            assert_eq!(
                parse_protocol_version(version)
                    .is_some_and(|version| version >= MIN_LIGHTWALLET_PROTOCOL_VERSION),
                supported,
                "{version}"
            );
        }
    }
}

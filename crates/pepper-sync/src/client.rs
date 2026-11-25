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
use zcash_protocol::consensus::{self, BlockHeight};

use zl_common::chain;
use zl_netutils::{Indexer, TransparentIndexer};

use crate::{
    error::{MempoolError, ServerError, stream_error_is_retryable},
    protocol,
    witness::Frontiers,
};

pub(crate) mod fetch;

const MAX_RETRIES: u8 = 3;

use zl_netutils::time::STREAM_MSG_TIMEOUT;

pub(crate) async fn next_stream_item<T>(
    stream: &mut protocol::Streaming<T>,
    timeout_message: &'static str,
) -> Result<Option<T>, protocol::Status> {
    match tokio::time::timeout(STREAM_MSG_TIMEOUT, stream.message()).await {
        Ok(res) => res,
        Err(_) => Err(protocol::Status::deadline_exceeded(timeout_message)),
    }
}

/// Fetch requests are created and sent to the [`crate::client::fetch::fetch`] task when a connection to the server is required.
///
/// Each variant includes a [`tokio::sync::oneshot::Sender`] for returning the fetched data to the requester.
#[derive(Debug)]
pub enum FetchRequest {
    /// Gets the height of the blockchain from the server.
    ChainTip(oneshot::Sender<Result<protocol::BlockId, protocol::Status>>),
    /// Gets  a compact block of the given block height.
    CompactBlock(
        oneshot::Sender<Result<protocol::CompactBlock, protocol::Status>>,
        BlockHeight,
    ),
    /// Gets the specified range of compact blocks from the server.
    ///
    /// Compact blocks include transparent data if the `bool` is true, otherwise only shielded data.
    CompactBlockRange(
        oneshot::Sender<Result<protocol::CompactBlocks, protocol::Status>>,
        protocol::BlockSpan,
        bool,
    ),
    /// Gets the specified range of nullifiers from the server.
    NullifierRange(
        oneshot::Sender<Result<protocol::CompactBlocks, protocol::Status>>,
        protocol::BlockSpan,
    ),
    /// Gets the tree states for a specified block height.
    TreeState(
        oneshot::Sender<Result<chain::TreeStateReply, protocol::Status>>,
        BlockHeight,
    ),
    /// Get a full transaction by txid.
    Transaction(
        oneshot::Sender<Result<protocol::RawTransaction, protocol::Status>>,
        TxId,
    ),
    /// Get a list of unspent transparent output metadata for a given list of transparent addresses and start height.
    #[allow(dead_code)]
    UtxoMetadata(
        oneshot::Sender<Result<Vec<protocol::GetAddressUtxosReply>, protocol::Status>>,
        (Vec<String>, BlockHeight),
    ),
    /// Get a list of transactions for a given transparent address and block range.
    TransparentAddressTxs(
        oneshot::Sender<Result<protocol::RawTransactions, protocol::Status>>,
        (String, protocol::BlockSpan),
    ),
    /// Get a stream of shards.
    SubtreeRoots(
        oneshot::Sender<Result<protocol::SubtreeRoots, protocol::Status>>,
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
) -> Result<protocol::CompactBlock, ServerError> {
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

/// Gets the specified range of compact blocks from the server.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub(crate) async fn get_compact_block_range(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    block_span: protocol::BlockSpan,
    include_transparent: bool,
) -> Result<protocol::CompactBlocks, ServerError> {
    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::CompactBlockRange(
            reply_sender,
            block_span,
            include_transparent,
        ))
        .map_err(|_| ServerError::FetcherDropped)?;

    let block_stream = reply_receiver
        .await
        .map_err(|_| ServerError::FetcherDropped)?
        .map_err(ServerError::RequestFailed)?;

    Ok(block_stream)
}

/// Gets the specified range of nullifiers from the server.
///
/// Nullifiers are stored in compact blocks where the actions contain only nullifiers.
///
/// Requires [`crate::client::fetch::fetch`] to be running concurrently, connected via the `fetch_request` channel.
pub(crate) async fn get_nullifier_range(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    block_span: protocol::BlockSpan,
) -> Result<protocol::CompactBlocks, ServerError> {
    let (reply_sender, reply_receiver) = oneshot::channel();
    fetch_request_sender
        .send(FetchRequest::NullifierRange(reply_sender, block_span))
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
) -> Result<Vec<protocol::SubtreeRoot>, ServerError> {
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

        while let Some(subtree_root) = match next_stream_item(
            &mut subtree_root_stream,
            "SubtreeRoots stream message timeout",
        )
        .await
        {
            Ok(s) => s,
            Err(e) if stream_error_is_retryable(&e) && retry_count < MAX_RETRIES => {
                tokio::time::sleep(Duration::from_secs(3)).await;
                retry_count += 1;
                continue 'retry;
            }
            Err(e) => return Err(e.into()),
        } {
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

    Frontiers::of_tree_state(&tree_state, consensus_parameters, block_height)
}

const SNAPSHOT_RETRY_HINT: &str = "retry with a fresh snapshot";

/// - Sends a transaction request to the fetcher, and sends it again after each
///   refusal that carries [`SNAPSHOT_RETRY_HINT`], at most [`MAX_RETRIES`] times.
pub(crate) async fn get_transaction_and_block_height(
    fetch_request_sender: UnboundedSender<FetchRequest>,
    consensus_parameters: &impl consensus::Parameters,
    txid: TxId,
) -> Result<(Transaction, BlockHeight), ServerError> {
    let mut retry_count = 0;
    let raw_transaction = loop {
        let (reply_sender, reply_receiver) = oneshot::channel();
        fetch_request_sender
            .send(FetchRequest::Transaction(reply_sender, txid))
            .map_err(|_| ServerError::FetcherDropped)?;

        match reply_receiver
            .await
            .map_err(|_| ServerError::FetcherDropped)?
        {
            Ok(raw_transaction) => break raw_transaction,
            Err(status)
                if status.message().contains(SNAPSHOT_RETRY_HINT) && retry_count < MAX_RETRIES =>
            {
                retry_count += 1;
            }
            Err(status) => return Err(ServerError::RequestFailed(status)),
        }
    };
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
) -> Result<Vec<protocol::GetAddressUtxosReply>, ServerError> {
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
    let Some(block_span) = protocol::BlockSpan::of(&block_range) else {
        return Ok(Vec::new());
    };
    let mut raw_transactions: Vec<protocol::RawTransaction> = Vec::new();
    let mut retry_count = 0;

    'retry: loop {
        let (reply_sender, reply_receiver) = oneshot::channel();

        fetch_request_sender
            .send(FetchRequest::TransparentAddressTxs(
                reply_sender,
                (transparent_address.clone(), block_span),
            ))
            .map_err(|_| ServerError::FetcherDropped)?;

        let mut raw_transaction_stream = reply_receiver
            .await
            .map_err(|_| ServerError::FetcherDropped)?
            .map_err(ServerError::RequestFailed)?;

        while let Some(raw_tx) = match next_stream_item(
            &mut raw_transaction_stream,
            "TransparentAddressTxs stream message timeout",
        )
        .await
        {
            Ok(s) => s,
            Err(e) if stream_error_is_retryable(&e) && retry_count < MAX_RETRIES => {
                tokio::time::sleep(Duration::from_secs(3)).await;
                retry_count += 1;
                raw_transactions.clear();
                continue 'retry;
            }
            Err(e) => return Err(e.into()),
        } {
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
) -> Result<protocol::RawTransactions, MempoolError>
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
    use std::sync::atomic;

    use tokio::sync::mpsc;
    use zcash_primitives::block;
    use zcash_protocol::{PoolType, ShieldedPool, local_consensus::LocalNetwork};

    use super::*;
    use crate::mocks;

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
        nu7: Some(BlockHeight::from_u32(IRONWOOD_ACTIVATION)),
    };

    /// Answers tree state requests with an empty tree for every pool except `omitted_pool`, whose tree state field
    /// is left empty.
    fn spawn_fetcher(omitted_pool: ShieldedPool) -> mpsc::UnboundedSender<FetchRequest> {
        mocks::spawn_tree_state_fetcher(move |block_height| {
            mocks::with_tree_hex(
                mocks::empty_trees(block_height),
                omitted_pool,
                mocks::OMITTED_TREE_HEX,
            )
        })
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

    const MALFORMED_HASH_HEX: &str = "zz";
    const SHORT_HASH_HEX: &str = "00ff";
    const HEIGHT_BEYOND_32_BITS: u64 = u32::MAX as u64 + 1;

    async fn frontiers_of(tree_state: protocol::TreeState) -> Result<Frontiers, ServerError> {
        get_frontiers(
            mocks::spawn_tree_state_fetcher(move |_| tree_state.clone()),
            &NETWORK,
            IRONWOOD_ACTIVATION.into(),
        )
        .await
    }

    const MARKED_BYTE: u8 = 0x01;
    const FIRST_BYTE: usize = 0;
    const LAST_BYTE: usize = mocks::BLOCK_HASH_BYTES - 1;

    #[tokio::test]
    async fn a_well_formed_tree_state_keeps_its_height_and_reverses_its_block_hash() {
        let mut wire_hash = [0; mocks::BLOCK_HASH_BYTES];
        wire_hash[FIRST_BYTE] = MARKED_BYTE;
        let mut reversed_hash = [0; mocks::BLOCK_HASH_BYTES];
        reversed_hash[LAST_BYTE] = MARKED_BYTE;

        let frontiers = frontiers_of(protocol::TreeState {
            hash: hex::encode(wire_hash),
            ..mocks::empty_trees(IRONWOOD_ACTIVATION.into())
        })
        .await;

        assert_eq!(
            frontiers
                .map(|frontiers| (frontiers.block_height(), frontiers.block_hash()))
                .map_err(|unread| unread.to_string()),
            Ok((IRONWOOD_ACTIVATION.into(), block::BlockHash(reversed_hash)))
        );
    }

    #[tokio::test]
    async fn each_malformed_field_keeps_its_report() {
        let well_formed = mocks::empty_trees(IRONWOOD_ACTIVATION.into());
        for (malformed, report) in [
            (
                protocol::TreeState {
                    hash: MALFORMED_HASH_HEX.to_string(),
                    ..well_formed.clone()
                },
                "Block hash is not valid hex",
            ),
            (
                protocol::TreeState {
                    height: HEIGHT_BEYOND_32_BITS,
                    ..well_formed.clone()
                },
                "Invalid block height",
            ),
            (
                protocol::TreeState {
                    hash: SHORT_HASH_HEX.to_string(),
                    ..well_formed.clone()
                },
                "Invalid block hash length.",
            ),
        ] {
            let frontiers = frontiers_of(malformed).await;

            assert!(
                matches!(
                    &frontiers,
                    Err(ServerError::InvalidFrontier(invalid))
                        if invalid.to_string().starts_with(report)
                ),
                "a malformed tree state was reported as {:?}",
                frontiers.map(|_| "valid frontiers")
            );
        }
    }

    #[tokio::test]
    async fn an_omitted_tree_is_reported_as_not_served_when_another_field_is_malformed() {
        let frontiers = frontiers_of(protocol::TreeState {
            hash: MALFORMED_HASH_HEX.to_string(),
            ironwood_tree: mocks::OMITTED_TREE_HEX.to_string(),
            ..mocks::empty_trees(IRONWOOD_ACTIVATION.into())
        })
        .await;

        assert!(
            matches!(
                frontiers,
                Err(ServerError::TreeStateNotServed {
                    pool: PoolType::Shielded(ShieldedPool::Ironwood),
                    height,
                }) if height == IRONWOOD_ACTIVATION.into()
            ),
            "an omitted Ironwood tree was reported as {:?}",
            frontiers.map(|_| "valid frontiers")
        );
    }

    const TRANSPARENT_ADDRESS: &str = "t1abc";

    #[tokio::test]
    async fn an_empty_block_range_holds_no_transparent_transaction() {
        let (fetch_request_sender, request_count) = mocks::spawn_request_counter();

        let transactions = get_transparent_address_transactions(
            fetch_request_sender,
            &NETWORK,
            TRANSPARENT_ADDRESS.to_string(),
            mocks::empty_block_range(),
        )
        .await;

        assert!(
            matches!(transactions.as_deref(), Ok([])),
            "an empty block range gave {:?}",
            transactions.map(|transactions| transactions.len())
        );
        assert_eq!(
            request_count.load(atomic::Ordering::Acquire),
            mocks::NO_REQUESTS,
            "an empty block range reached the fetcher"
        );
    }

    /// Answers transaction requests with each status of `replies` in turn, then
    /// panics, so a caller that asks more often than the script allows fails.
    fn spawn_transaction_fetcher(
        replies: Vec<protocol::Status>,
    ) -> mpsc::UnboundedSender<FetchRequest> {
        let mut replies = replies.into_iter();
        mocks::spawn_transaction_fetcher(move |_txid| {
            Some(Err(replies.next().expect(
                "the test scripts one reply for each request it makes",
            )))
        })
    }

    fn snapshot_race() -> protocol::Status {
        protocol::Status::unknown(
            "Chain index error: unavailable: mempool is not coherent with the requested \
             snapshot; retry with a fresh snapshot",
        )
    }

    fn status_of(result: Result<(Transaction, BlockHeight), ServerError>) -> protocol::Status {
        match result {
            Err(ServerError::RequestFailed(status)) => status,
            other => panic!("expected a failed request, got {other:?}"),
        }
    }

    /// HYPOTHESIS: zaino answers a transaction request caught between two
    /// mempool snapshots with an `Unknown` status that asks the client to
    /// retry, and the client obeys, up to the retry ceiling. Falsified if the
    /// first such status surfaces, or if the ceiling is not enforced.
    #[tokio::test]
    async fn a_snapshot_race_is_retried_up_to_the_ceiling() {
        let within_ceiling: Vec<protocol::Status> = (0..MAX_RETRIES)
            .map(|_| snapshot_race())
            .chain([protocol::Status::not_found("gone")])
            .collect();
        let status = status_of(
            get_transaction_and_block_height(
                spawn_transaction_fetcher(within_ceiling),
                &NETWORK,
                TxId::from_bytes([0; 32]),
            )
            .await,
        );
        assert_eq!(status.code(), protocol::Code::NotFound, "{status}");

        let beyond_ceiling: Vec<protocol::Status> = (0..=MAX_RETRIES)
            .map(|_| snapshot_race())
            .chain([protocol::Status::not_found("never asked")])
            .collect();
        let status = status_of(
            get_transaction_and_block_height(
                spawn_transaction_fetcher(beyond_ceiling),
                &NETWORK,
                TxId::from_bytes([0; 32]),
            )
            .await,
        );
        assert_eq!(status.code(), protocol::Code::Unknown, "{status}");
    }

    /// HYPOTHESIS: any other failed request surfaces at once. Falsified if a
    /// second request is made.
    #[tokio::test]
    async fn an_unrelated_failure_is_not_retried() {
        let status = status_of(
            get_transaction_and_block_height(
                spawn_transaction_fetcher(vec![protocol::Status::internal("broken")]),
                &NETWORK,
                TxId::from_bytes([0; 32]),
            )
            .await,
        );
        assert_eq!(status.code(), protocol::Code::Internal, "{status}");
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

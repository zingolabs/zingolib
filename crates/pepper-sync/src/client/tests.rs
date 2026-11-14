use tokio::sync::mpsc;
use zcash_protocol::local_consensus::LocalNetwork;
use zl_netutils::lightwallet_protocol::TreeState;

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
    nu7: Some(BlockHeight::from_u32(IRONWOOD_ACTIVATION)),
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

/// Answers transaction requests with each status of `replies` in turn, then
/// panics, so a caller that asks more often than the script allows fails.
fn spawn_transaction_fetcher(replies: Vec<tonic::Status>) -> mpsc::UnboundedSender<FetchRequest> {
    let (fetch_request_sender, mut fetch_request_receiver) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut replies = replies.into_iter();
        while let Some(fetch_request) = fetch_request_receiver.recv().await {
            match fetch_request {
                FetchRequest::Transaction(reply_sender, _) => {
                    let status = replies.next().expect("a request beyond the script");
                    let _ignore_error = reply_sender.send(Err(status));
                }
                _ => panic!("unexpected fetch request"),
            }
        }
    });
    fetch_request_sender
}

fn snapshot_race() -> tonic::Status {
    tonic::Status::unknown(
        "Chain index error: unavailable: mempool is not coherent with the requested \
             snapshot; retry with a fresh snapshot",
    )
}

fn status_of(result: Result<(Transaction, BlockHeight), ServerError>) -> tonic::Status {
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
    let within_ceiling: Vec<tonic::Status> = (0..MAX_RETRIES)
        .map(|_| snapshot_race())
        .chain([tonic::Status::not_found("gone")])
        .collect();
    let status = status_of(
        get_transaction_and_block_height(
            spawn_transaction_fetcher(within_ceiling),
            &NETWORK,
            TxId::from_bytes([0; 32]),
        )
        .await,
    );
    assert_eq!(status.code(), tonic::Code::NotFound, "{status}");

    let beyond_ceiling: Vec<tonic::Status> = (0..=MAX_RETRIES)
        .map(|_| snapshot_race())
        .chain([tonic::Status::not_found("never asked")])
        .collect();
    let status = status_of(
        get_transaction_and_block_height(
            spawn_transaction_fetcher(beyond_ceiling),
            &NETWORK,
            TxId::from_bytes([0; 32]),
        )
        .await,
    );
    assert_eq!(status.code(), tonic::Code::Unknown, "{status}");
}

/// HYPOTHESIS: any other failed request surfaces at once. Falsified if a
/// second request is made.
#[tokio::test]
async fn an_unrelated_failure_is_not_retried() {
    let status = status_of(
        get_transaction_and_block_height(
            spawn_transaction_fetcher(vec![tonic::Status::internal("broken")]),
            &NETWORK,
            TxId::from_bytes([0; 32]),
        )
        .await,
    );
    assert_eq!(status.code(), tonic::Code::Internal, "{status}");
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

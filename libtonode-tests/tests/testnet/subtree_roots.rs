use std::num::NonZeroU32;
use std::time::Duration;

use incrementalmerkletree::Position;
use pepper_sync::error::SyncError;
use pepper_sync::wallet::ShardTrees;
use shardtree::ShardTree;
use shardtree::store::ShardStore;
use zcash_protocol::ShieldedPool;
use zingo_netutils::lightwallet_protocol::{GetSubtreeRootsArg, ShieldedProtocol};
use zingo_netutils::{GrpcIndexer, Indexer};
use zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;
use zingolib::config::{ChainType, ClientConfig, WalletConfig, construct_indexer_uri};
use zingolib::data::PollReport;
use zingolib::lightclient::error::LightClientError;
use zingolib::lightclient::{DEFAULT_REQUEST_TIMEOUT, LightClient};
use zingolib::sync::SyncModeError;
use zingolib::testutils::chain_generics::conduct_chain::ConductChain;
use zingolib::testutils::chain_generics::networked::NetworkedTestEnvironment;
use zingolib::testutils::{default_test_wallet_settings, tempfile::TempDir};

use super::TESTNET_INDEXER;

const BIRTHDAY_DEPTH: u32 = 100;
const SESSION_POLL: Duration = Duration::from_secs(1);
const POOLS: [ShieldedPool; 3] = [
    ShieldedPool::Sapling,
    ShieldedPool::Orchard,
    ShieldedPool::Ironwood,
];

fn shielded_protocol(pool: ShieldedPool) -> ShieldedProtocol {
    match pool {
        ShieldedPool::Sapling => ShieldedProtocol::Sapling,
        ShieldedPool::Orchard => ShieldedProtocol::Orchard,
        ShieldedPool::Ironwood => ShieldedProtocol::Ironwood,
    }
}

/// The wallet's shard roots for `tree`, encoded like the server's, with one
/// trailing root above the server's count dropped: the wallet's open shard.
fn wallet_roots<S, const DEPTH: u8, const SHARD_HEIGHT: u8>(
    tree: &ShardTree<S, DEPTH, SHARD_HEIGHT>,
    server_count: usize,
    encode: impl Fn(&S::H) -> Vec<u8>,
) -> Vec<Vec<u8>>
where
    S: ShardStore,
    S::Error: std::fmt::Debug,
    S::H: incrementalmerkletree::Hashable + Clone + PartialEq,
    S::CheckpointId: Clone + Ord + std::fmt::Debug,
{
    let mut addresses = tree
        .store()
        .get_shard_roots()
        .expect("the in-memory shard store lists its roots");
    if addresses.len() > server_count {
        addresses.pop();
    }
    addresses
        .into_iter()
        .map(|address| {
            encode(
                &tree
                    .root(address, Position::from(u64::MAX))
                    .expect("a stored shard has a root"),
            )
        })
        .collect()
}

/// - Truncates `tree` to half the server's roots, so the next session has to fetch the rest again.
fn cut_to_half<S, const DEPTH: u8, const SHARD_HEIGHT: u8>(
    tree: &mut ShardTree<S, DEPTH, SHARD_HEIGHT>,
    server_count: usize,
) -> usize
where
    S: ShardStore,
    S::Error: std::fmt::Debug,
    S::H: incrementalmerkletree::Hashable + Clone + PartialEq,
    S::CheckpointId: Clone + Ord + std::fmt::Debug,
{
    tree.store_mut()
        .truncate_shards((server_count / 2) as u64)
        .expect("the in-memory shard store truncates");
    tree.store()
        .get_shard_roots()
        .expect("the in-memory shard store lists its roots")
        .len()
}

fn assert_roots_match_server(shard_trees: &ShardTrees, server: &[(ShieldedPool, Vec<Vec<u8>>)]) {
    for (pool, server_roots) in server {
        let wallet_roots = match pool {
            ShieldedPool::Sapling => {
                wallet_roots(&shard_trees.sapling, server_roots.len(), |root| {
                    root.to_bytes().to_vec()
                })
            }
            ShieldedPool::Orchard => {
                wallet_roots(&shard_trees.orchard, server_roots.len(), |root| {
                    root.to_bytes().to_vec()
                })
            }
            ShieldedPool::Ironwood => {
                wallet_roots(&shard_trees.ironwood, server_roots.len(), |root| {
                    root.to_bytes().to_vec()
                })
            }
        };
        assert!(
            !server_roots.is_empty(),
            "the server serves at least one {pool:?} subtree root"
        );
        assert_eq!(
            &wallet_roots, server_roots,
            "{pool:?} shard roots in the wallet versus on the server"
        );
    }
}

async fn server_roots(grpc_client: &mut GrpcIndexer, pool: ShieldedPool) -> Vec<Vec<u8>> {
    let mut roots = Vec::new();
    let mut stream = grpc_client
        .get_subtree_roots(
            GetSubtreeRootsArg {
                start_index: 0,
                shielded_protocol: shielded_protocol(pool) as i32,
                max_entries: 0,
            },
            DEFAULT_REQUEST_TIMEOUT,
        )
        .await
        .expect("the indexer streams its subtree roots");
    while let Some(root) = stream
        .message()
        .await
        .expect("the subtree root stream delivers every root")
    {
        roots.push(root.root_hash);
    }
    roots
}

/// - Runs one sync session to its first status, then stops it and awaits its return.
async fn session_start_then_stop(lightclient: &mut LightClient) {
    lightclient.sync().await.expect("the sync session launches");
    // The session's first status is published once its start, which fetches
    // the subtree roots, is complete.
    while !(lightclient.latest_sync_status().is_some()
        || matches!(lightclient.poll_sync(), PollReport::Ready(_)))
    {
        tokio::time::sleep(SESSION_POLL).await;
    }
    let _ = lightclient.stop_sync();
    match lightclient.await_sync().await {
        Ok(_)
        | Err(LightClientError::SyncError(SyncError::SyncModeError(
            SyncModeError::SyncNotRunning,
        )))
        | Err(LightClientError::SyncNotRunning) => {}
        Err(error) => panic!("{error}"),
    }
}

#[tokio::test]
async fn add_subtree_roots() {
    zingolib::ensure_default_crypto_provider();

    let uri = construct_indexer_uri(TESTNET_INDEXER.to_string())
        .expect("the hard-coded testnet indexer constant is a valid URI");
    let mut grpc_client = GrpcIndexer::new(uri.clone())
        .await
        .expect("the testnet indexer accepts a connection");
    let tip = grpc_client
        .get_latest_block(DEFAULT_REQUEST_TIMEOUT)
        .await
        .expect("the testnet indexer serves its latest block")
        .height as u32;

    let temp_dir = TempDir::new().expect("the host provides a temporary wallet directory");
    let config = ClientConfig::builder()
        .set_indexer_uri(uri)
        .set_chain_type(ChainType::Testnet)
        .set_wallet_dir(temp_dir.path().to_path_buf())
        .set_wallet_config(WalletConfig::MnemonicPhrase {
            mnemonic_phrase: HOSPITAL_MUSEUM_SEED.to_string(),
            no_of_accounts: NonZeroU32::try_from(1).expect("hard-coded integer"),
            birthday: tip - BIRTHDAY_DEPTH,
            wallet_settings: default_test_wallet_settings(),
        })
        .build()
        .expect("every required field of the config is set above");
    let mut lightclient = LightClient::new(config, true)
        .await
        .expect("a fresh testnet client opens");

    let mut server = Vec::new();
    for pool in POOLS {
        server.push((pool, server_roots(&mut grpc_client, pool).await));
    }

    session_start_then_stop(&mut lightclient).await;
    {
        let shard_trees = &mut lightclient.wallet().write().await.shard_trees;
        assert_roots_match_server(shard_trees, &server);
        for (pool, server_roots) in &server {
            let remaining = match pool {
                ShieldedPool::Sapling => cut_to_half(&mut shard_trees.sapling, server_roots.len()),
                ShieldedPool::Orchard => cut_to_half(&mut shard_trees.orchard, server_roots.len()),
                ShieldedPool::Ironwood => {
                    cut_to_half(&mut shard_trees.ironwood, server_roots.len())
                }
            };
            assert_ne!(
                remaining,
                server_roots.len(),
                "cutting the {pool:?} tree must leave it short of the server"
            );
        }
    }

    // A new block makes the next session fetch the subtree roots again.
    NetworkedTestEnvironment::setup()
        .await
        .increase_chain_height()
        .await;

    session_start_then_stop(&mut lightclient).await;
    let shard_trees = &lightclient.wallet().read().await.shard_trees;
    assert_roots_match_server(shard_trees, &server);
}

use std::num::NonZeroU32;
use std::time::Duration;

use incrementalmerkletree::Position;
use pepper_sync::error::SyncError;
use pepper_sync::wallet::ShardTrees;
use shardtree::store::ShardStore;
use zingo_netutils::lightwallet_protocol::GetSubtreeRootsArg;
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
const SAPLING_PROTOCOL: i32 = 0;
const ORCHARD_PROTOCOL: i32 = 1;
const SESSION_POLL: Duration = Duration::from_secs(1);

fn assert_subtree_roots_match_server(
    shard_trees: &mut ShardTrees,
    sapling_subtree_roots_server: &[Vec<u8>],
    orchard_subtree_roots_server: &[Vec<u8>],
) {
    let mut sapling_shard_addrs = shard_trees
        .sapling
        .store()
        .get_shard_roots()
        .expect("the in-memory shard store lists its roots");
    if sapling_shard_addrs.len() > sapling_subtree_roots_server.len() {
        sapling_shard_addrs.pop();
    }
    assert_eq!(
        sapling_shard_addrs.len(),
        sapling_subtree_roots_server.len(),
        "sapling shard roots in the wallet versus on the server"
    );
    let sapling_subtree_roots_wallet: Vec<Vec<u8>> = sapling_shard_addrs
        .into_iter()
        .map(|addr| {
            shard_trees
                .sapling
                .root(addr, Position::from(u64::MAX))
                .expect("a stored shard has a root")
                .to_bytes()
                .to_vec()
        })
        .collect();
    assert!(!sapling_subtree_roots_server.is_empty());
    assert_eq!(
        sapling_subtree_roots_wallet.len(),
        sapling_subtree_roots_server.len()
    );
    for (wallet_root, server_root) in sapling_subtree_roots_wallet
        .iter()
        .zip(sapling_subtree_roots_server)
    {
        assert_eq!(wallet_root, server_root);
    }

    let mut orchard_shard_addrs = shard_trees
        .orchard
        .store()
        .get_shard_roots()
        .expect("the in-memory shard store lists its roots");
    if orchard_shard_addrs.len() > orchard_subtree_roots_server.len() {
        orchard_shard_addrs.pop();
    }
    assert_eq!(
        orchard_shard_addrs.len(),
        orchard_subtree_roots_server.len(),
        "orchard shard roots in the wallet versus on the server"
    );
    let orchard_subtree_roots_wallet: Vec<Vec<u8>> = orchard_shard_addrs
        .into_iter()
        .map(|addr| {
            shard_trees
                .orchard
                .root(addr, Position::from(u64::MAX))
                .expect("a stored shard has a root")
                .to_bytes()
                .to_vec()
        })
        .collect();
    assert!(!orchard_subtree_roots_server.is_empty());
    assert_eq!(
        orchard_subtree_roots_wallet.len(),
        orchard_subtree_roots_server.len()
    );
    for (wallet_root, server_root) in orchard_subtree_roots_wallet
        .iter()
        .zip(orchard_subtree_roots_server)
    {
        assert_eq!(wallet_root, server_root);
    }
}

async fn server_subtree_roots(
    grpc_client: &mut GrpcIndexer,
    shielded_protocol: i32,
) -> Vec<Vec<u8>> {
    let mut roots = Vec::new();
    let mut stream = grpc_client
        .get_subtree_roots(
            GetSubtreeRootsArg {
                start_index: 0,
                shielded_protocol,
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

    let sapling_subtree_roots_server =
        server_subtree_roots(&mut grpc_client, SAPLING_PROTOCOL).await;
    let orchard_subtree_roots_server =
        server_subtree_roots(&mut grpc_client, ORCHARD_PROTOCOL).await;

    session_start_then_stop(&mut lightclient).await;
    {
        let shard_trees = &mut lightclient.wallet().write().await.shard_trees;
        assert_subtree_roots_match_server(
            shard_trees,
            &sapling_subtree_roots_server,
            &orchard_subtree_roots_server,
        );

        // Cut each tree to half the server's roots, so the next session has to
        // fetch the rest again.
        let sapling_cut = (sapling_subtree_roots_server.len() / 2) as u64;
        shard_trees
            .sapling
            .store_mut()
            .truncate_shards(sapling_cut)
            .expect("the in-memory shard store truncates");
        let sapling_shard_addrs = shard_trees
            .sapling
            .store()
            .get_shard_roots()
            .expect("the in-memory shard store lists its roots");
        assert_ne!(
            sapling_shard_addrs.len(),
            sapling_subtree_roots_server.len()
        );

        let orchard_cut = (orchard_subtree_roots_server.len() / 2) as u64;
        shard_trees
            .orchard
            .store_mut()
            .truncate_shards(orchard_cut)
            .expect("the in-memory shard store truncates");
        let orchard_shard_addrs = shard_trees
            .orchard
            .store()
            .get_shard_roots()
            .expect("the in-memory shard store lists its roots");
        assert_ne!(
            orchard_shard_addrs.len(),
            orchard_subtree_roots_server.len()
        );
    }

    // A new block makes the next session fetch the subtree roots again.
    NetworkedTestEnvironment::setup()
        .await
        .increase_chain_height()
        .await;

    session_start_then_stop(&mut lightclient).await;
    {
        let shard_trees = &mut lightclient.wallet().write().await.shard_trees;
        assert_subtree_roots_match_server(
            shard_trees,
            &sapling_subtree_roots_server,
            &orchard_subtree_roots_server,
        );
    }
}

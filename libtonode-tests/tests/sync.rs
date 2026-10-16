use std::{num::NonZeroU32, time::Duration};

use incrementalmerkletree::Position;
use pepper_sync::error::SyncError;
use pepper_sync::test_support::block;
use pepper_sync::wallet::ShardTrees;
use shardtree::store::ShardStore;
use zcash_local_net::validator::Validator;
use zcash_protocol::PoolType;
use zcash_protocol::ShieldedPool::{Ironwood, Orchard, Sapling};
use zingo_netutils::lightwallet_protocol::{BlockId, BlockRange, GetSubtreeRootsArg};
use zingo_netutils::{GrpcIndexer, Indexer};
use zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;
use zingolib::config::{ChainType, ClientConfig, WalletConfig};
use zingolib::data::PollReport;
use zingolib::lightclient::DEFAULT_REQUEST_TIMEOUT;
use zingolib::lightclient::error::LightClientError;
use zingolib::sync::SyncModeError;
use zingolib::testutils::default_test_wallet_settings;
use zingolib::testutils::tempfile::TempDir;
use zingolib::{
    config::construct_indexer_uri,
    get_base_address_macro,
    lightclient::LightClient,
    testutils::lightclient::from_inputs::{self},
};

/// The mainnet indexer these ignored, hand-run sync tests dial explicitly.
const SYNC_TEST_INDEXER: &str = "https://zec.rocks:443";
use zingolib_testutils::scenarios;


// TODO: migrate to mock test as this test is long and connects to mainnet
#[tokio::test]
async fn add_subtree_roots() {
    fn assert_subtree_roots_match_server(
        shard_trees: &mut ShardTrees,
        sapling_subtree_roots_server: Vec<Vec<u8>>,
        orchard_subtree_roots_server: Vec<Vec<u8>>,
    ) {
        let mut sapling_shard_addrs = shard_trees.sapling.store().get_shard_roots().unwrap();
        if sapling_shard_addrs.len() > sapling_subtree_roots_server.len() {
            sapling_shard_addrs.pop();
        }
        assert!(
            sapling_shard_addrs.len() == sapling_subtree_roots_server.len(),
            "no of sapling shard roots wallet: {}, no of sapling shard roots server: {}",
            sapling_shard_addrs.len(),
            sapling_subtree_roots_server.len()
        );
        let mut sapling_subtree_roots_wallet = Vec::new();
        for addr in sapling_shard_addrs {
            let root = shard_trees
                .sapling
                .root(addr, Position::from(u64::MAX))
                .unwrap();
            sapling_subtree_roots_wallet.push(root.to_bytes().to_vec());
        }

        assert!(!sapling_subtree_roots_server.is_empty());
        assert!(
            sapling_subtree_roots_wallet.len() == sapling_subtree_roots_server.len(),
            "no of sapling shard roots wallet: {}, no of sapling shard roots server: {}",
            sapling_subtree_roots_wallet.len(),
            sapling_subtree_roots_server.len()
        );
        sapling_subtree_roots_wallet
            .iter()
            .zip(sapling_subtree_roots_server.clone())
            .for_each(|(wallet_root, server_root)| {
                assert!(wallet_root.as_slice() == server_root.as_slice());
            });

        let mut orchard_shard_addrs = shard_trees.orchard.store().get_shard_roots().unwrap();
        if orchard_shard_addrs.len() > orchard_subtree_roots_server.len() {
            orchard_shard_addrs.pop();
        }
        assert!(
            orchard_shard_addrs.len() == orchard_subtree_roots_server.len(),
            "no of orchard shard roots wallet: {}, no of orchard shard roots server: {}",
            orchard_shard_addrs.len(),
            orchard_subtree_roots_server.len()
        );
        let mut orchard_subtree_roots_wallet = Vec::new();
        for addr in orchard_shard_addrs {
            let root = shard_trees
                .orchard
                .root(addr, Position::from(u64::MAX))
                .unwrap();
            orchard_subtree_roots_wallet.push(root.to_bytes().to_vec());
        }

        assert!(!orchard_subtree_roots_server.is_empty());
        assert!(
            orchard_subtree_roots_wallet.len() == orchard_subtree_roots_server.len(),
            "no of orchard shard roots wallet: {}, no of orchard shard roots server: {}",
            orchard_subtree_roots_wallet.len(),
            orchard_subtree_roots_server.len()
        );
        orchard_subtree_roots_wallet
            .iter()
            .zip(orchard_subtree_roots_server)
            .for_each(|(wallet_root, server_root)| {
                assert!(wallet_root.as_slice() == server_root.as_slice());
            });
    }

    zingolib::ensure_default_crypto_provider();

    let uri = construct_indexer_uri(SYNC_TEST_INDEXER.to_string()).unwrap();
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();
    let config = ClientConfig::builder()
        .set_indexer_uri(uri.clone())
        .set_chain_type(ChainType::Mainnet)
        .set_wallet_dir(temp_path)
        .set_wallet_config(WalletConfig::MnemonicPhrase {
            mnemonic_phrase: HOSPITAL_MUSEUM_SEED.to_string(),
            no_of_accounts: NonZeroU32::try_from(1).expect("hard-coded integer"),
            birthday: 2_000_000,
            wallet_settings: default_test_wallet_settings(),
        })
        .build()
        .unwrap();
    let mut lightclient = LightClient::new(config, true).await.unwrap();

    let mut grpc_client = GrpcIndexer::new(
        lightclient
            .indexer_uri()
            .expect("live-net client is connected"),
    )
    .await
    .unwrap();

    let mut sapling_subtree_roots_server = Vec::new();
    let mut sapling_subtree_roots_stream = grpc_client
        .get_subtree_roots(
            GetSubtreeRootsArg {
                start_index: 0,
                shielded_protocol: 0,
                max_entries: 0,
            },
            DEFAULT_REQUEST_TIMEOUT,
        )
        .await
        .unwrap();
    while let Some(root) = sapling_subtree_roots_stream.message().await.unwrap() {
        sapling_subtree_roots_server.push(root.root_hash);
    }

    let mut orchard_subtree_roots_server = Vec::new();
    let mut orchard_subtree_roots_stream = grpc_client
        .get_subtree_roots(
            GetSubtreeRootsArg {
                start_index: 0,
                shielded_protocol: 1,
                max_entries: 0,
            },
            DEFAULT_REQUEST_TIMEOUT,
        )
        .await
        .unwrap();
    while let Some(root) = orchard_subtree_roots_stream.message().await.unwrap() {
        orchard_subtree_roots_server.push(root.root_hash);
    }

    lightclient.sync().await.unwrap();
    // the session's first status is published once its start, which fetches the subtree roots, is complete. a
    // stop requested earlier abandons the session start, and a `Scanning` range the previous session left behind
    // would ask for one.
    while !(lightclient.latest_sync_status().is_some()
        || matches!(lightclient.poll_sync(), PollReport::Ready(_)))
    {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let _ = lightclient.stop_sync();
    match lightclient.await_sync().await {
        Ok(_) => {}
        Err(LightClientError::SyncError(SyncError::SyncModeError(
            SyncModeError::SyncNotRunning,
        )))
        | Err(LightClientError::SyncNotRunning) => {}
        Err(e) => {
            panic!("{e}");
        }
    }

    {
        let shard_trees = &mut lightclient.wallet().write().await.shard_trees;
        assert_subtree_roots_match_server(
            shard_trees,
            sapling_subtree_roots_server.clone(),
            orchard_subtree_roots_server.clone(),
        );

        shard_trees
            .sapling
            .store_mut()
            .truncate_shards(500)
            .unwrap();
        let sapling_shard_addrs = shard_trees.sapling.store().get_shard_roots().unwrap();
        assert!(sapling_shard_addrs.len() != sapling_subtree_roots_server.len());

        shard_trees
            .orchard
            .store_mut()
            .truncate_shards(500)
            .unwrap();
        let orchard_shard_addrs = shard_trees.orchard.store().get_shard_roots().unwrap();
        assert!(orchard_shard_addrs.len() != orchard_subtree_roots_server.len());
    }

    // must wait for a new block to be mined to trigger get_subtree_roots in sync
    tokio::time::sleep(Duration::from_secs(100)).await;

    lightclient.sync().await.unwrap();
    // the session's first status is published once its start, which fetches the subtree roots, is complete. a
    // stop requested earlier abandons the session start, and a `Scanning` range the previous session left behind
    // would ask for one.
    while !(lightclient.latest_sync_status().is_some()
        || matches!(lightclient.poll_sync(), PollReport::Ready(_)))
    {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let _ = lightclient.stop_sync();
    match lightclient.await_sync().await {
        Ok(_) => {}
        Err(LightClientError::SyncError(SyncError::SyncModeError(
            SyncModeError::SyncNotRunning,
        )))
        | Err(LightClientError::SyncNotRunning) => {}
        Err(e) => {
            panic!("{e}");
        }
    }

    {
        let shard_trees = &mut lightclient.wallet().write().await.shard_trees;

        assert_subtree_roots_match_server(
            shard_trees,
            sapling_subtree_roots_server.clone(),
            orchard_subtree_roots_server.clone(),
        );
    }
}

// temporary test for sync development
#[ignore = "sync development only"]
#[allow(unused_mut, unused_variables)]
#[tokio::test]
async fn sync_test() {
    tracing_subscriber::fmt().init();

    let (_local_net, mut faucet, mut recipient, _txid) =
        scenarios::faucet_funded_recipient_default(5_000_000).await;

    // let recipient_ua = get_base_address_macro!(&recipient, "unified");
    let recipient_taddr = get_base_address_macro!(&recipient, "transparent");
    from_inputs::quick_send(&mut faucet, vec![(&recipient_taddr, 100_000, None)])
        .await
        .unwrap();

    recipient.sync_and_await().await.unwrap();

    // increase_height_and_wait_for_client(&regtest_manager, &mut recipient, 1)
    //     .await
    //     .unwrap();

    // tracing::info!("{}", recipient.transaction_summaries().await.unwrap());
    tracing::info!("{}", recipient.value_transfers(false).await.unwrap());
    tracing::info!(
        "{}",
        recipient
            .account_balance(zip32::AccountId::ZERO)
            .await
            .unwrap()
    );
    tracing::info!(
        "{:?}",
        recipient.propose_shield(zip32::AccountId::ZERO).await
    );

    // tracing::info!(
    //     "{:?}",
    //     recipient
    //         .get_spendable_shielded_balance(
    //             zcash_address::ZcashAddress::try_from_encoded(&recipient_ua).unwrap(),
    //             false
    //         )
    //         .await
    //         .unwrap()
    // );
    // let wallet = recipient.wallet.lock().await;
    // dbg!(wallet.wallet_blocks.len());
}

/// Diagnostic for the container-only `add_subtree_roots` failure (wallet
/// consistently 32 sapling shards short of the server's count, 1096 vs
/// 1128).
///
/// Reproduction record (issue #2440): the failures occurred only in
/// container runs (twice, the wallet at exactly 1096 both times, under
/// `makers container-test`, podman on the originating machine) while
/// host runs of the same commit passed. "Container-only" records where
/// it happened, not a mechanism: in the same failing container process
/// a parallel fresh connection to the same URI saw all 1128 roots, so
/// the healthy backend was reachable from inside the container, and a
/// backend's state is client-independent. What pinned the wallet's
/// channel to the stuck backend in exactly the container runs is
/// unresolved, though logging the resolved backend identity (the issue's open
/// question) would make a recurrence attributable. Note the
/// stream-resume defense of 9d40495b9 converts mid-flight cuts into
/// completions but cannot conjure shards a stuck backend does not have,
/// so present-day non-reproduction indicates a healthy backend pool
/// (a day-later container run pulled 1128 cleanly), not a client fix.
///
/// Prints, for three fresh connections, how many sapling subtree roots the
/// stream yields and the chain height at which it ends, then the count
/// pepper-sync's own fetch path lands in the wallet. If a short stream's last
/// completing height is months old, the connection reached a lagging backend
/// behind the load balancer. If counts differ between connections to a
/// current backend, the stream is being cut mid-flight.
///
/// Run in the environment under investigation with output visible, e.g.:
///   makers test -p libtonode-tests \
///     -E 'test(diagnose_subtree_root_stream)' --run-ignored all --no-capture
#[ignore = "diagnostic: run manually against live mainnet"]
#[tokio::test]
async fn diagnose_subtree_root_stream() {
    let uri = construct_indexer_uri(SYNC_TEST_INDEXER.to_string()).unwrap();

    for attempt in 1..=3 {
        let mut grpc_client = GrpcIndexer::new(uri.clone()).await.unwrap();
        let mut stream = grpc_client
            .get_subtree_roots(
                GetSubtreeRootsArg {
                    start_index: 0,
                    shielded_protocol: 0,
                    max_entries: 0,
                },
                DEFAULT_REQUEST_TIMEOUT,
            )
            .await
            .unwrap();
        let mut count: u64 = 0;
        let mut last_completing_height: u64 = 0;
        let ending = loop {
            match stream.message().await {
                Ok(Some(root)) => {
                    count += 1;
                    last_completing_height = root.completing_block_height;
                }
                Ok(None) => break "clean end".to_string(),
                Err(status) => break format!("error: {status}"),
            }
        };
        println!(
            "connection {attempt}: {count} sapling subtree roots, \
             last completing height {last_completing_height}, ending: {ending}"
        );

        // Decisive follow-up: resume from where the stream ended, on the same
        // connection. A backend that genuinely has only `count` roots returns
        // 0 more; a stream that was cut mid-flight returns the remainder.
        let mut resume_stream = grpc_client
            .get_subtree_roots(
                GetSubtreeRootsArg {
                    start_index: u32::try_from(count).unwrap(),
                    shielded_protocol: 0,
                    max_entries: 0,
                },
                DEFAULT_REQUEST_TIMEOUT,
            )
            .await
            .unwrap();
        let mut resumed: u64 = 0;
        let resume_ending = loop {
            match resume_stream.message().await {
                Ok(Some(_)) => resumed += 1,
                Ok(None) => break "clean end".to_string(),
                Err(status) => break format!("error: {status}"),
            }
        };
        println!(
            "connection {attempt}: resume from index {count} yielded {resumed} more \
             roots, ending: {resume_ending}"
        );
    }

    // pepper-sync's own fetch path, exactly as `add_subtree_roots` drives it.
    let temp_dir = TempDir::new().unwrap();
    let config = ClientConfig::builder()
        .set_indexer_uri(uri)
        .set_chain_type(ChainType::Mainnet)
        .set_wallet_dir(temp_dir.path().to_path_buf())
        .set_wallet_config(WalletConfig::MnemonicPhrase {
            mnemonic_phrase: HOSPITAL_MUSEUM_SEED.to_string(),
            no_of_accounts: NonZeroU32::try_from(1).expect("hard-coded integer"),
            birthday: 2_000_000,
            wallet_settings: default_test_wallet_settings(),
        })
        .build()
        .unwrap();
    let mut lightclient = LightClient::new(config, true).await.unwrap();
    lightclient.sync().await.unwrap();
    // the session's first status is published once its start, which fetches the subtree roots, is complete. a
    // stop requested earlier abandons the session start, and a `Scanning` range the previous session left behind
    // would ask for one.
    while !(lightclient.latest_sync_status().is_some()
        || matches!(lightclient.poll_sync(), PollReport::Ready(_)))
    {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let _ = lightclient.stop_sync();
    let _ = lightclient.await_sync().await;

    let wallet = lightclient.wallet().write().await;
    let shard_addrs = wallet
        .shard_trees
        .sapling
        .store()
        .get_shard_roots()
        .unwrap();
    println!(
        "wallet (pepper-sync fetch path): {} sapling shard roots",
        shard_addrs.len()
    );
}

/// The indexer tip-lag race behind the historical
/// `unified_address_discovery` failure: after raw `generate_blocks`
/// returns, the *validator* has the new block while the *indexer*'s
/// poll-based ingestion reports the old tip for up to a few hundred
/// milliseconds (measured 10/10 rounds, max 475ms, on zainod 0.4.3 and
/// 0.6.0 alike). This test asserts the fix: after
/// `await_indexer_convergence` (the zcash_local_net barrier this crate's
/// helpers now route through), the indexer's served tip matches the
/// validator's in every round.
///
/// Ignored while zaino#1386 is open: zainod's "Syncing block" log line
/// is not linearized with its serving path, so the log-based barrier
/// can report satisfied while the gRPC surface still serves the prior
/// tip (observed flaking on both runtime flavors, always as "served
/// tip N-1 behind validator tip N"). Re-enable as the verification of
/// the zaino fix.
#[ignore = "zaino#1386: indexer log-vs-serve gap makes this flaky; re-enable on fix"]
#[tokio::test]
async fn indexer_converges_with_validator_after_block_generation() {
    let (local_net, client) = scenarios::unfunded_client_default().await;
    let mut grpc = GrpcIndexer::new(client.indexer_uri().expect("live-net client is connected"))
        .await
        .unwrap();

    const ROUNDS: u32 = 10;
    for round in 0..ROUNDS {
        local_net.validator().generate_blocks(1).await.unwrap();
        let validator_height = u64::from(local_net.validator().get_chain_height().await);
        local_net
            .await_indexer_convergence(validator_height as u32)
            .await
            .unwrap();
        let indexer_height = grpc
            .get_latest_block(DEFAULT_REQUEST_TIMEOUT)
            .await
            .unwrap()
            .height;
        assert!(
            indexer_height >= validator_height,
            "round {round}: indexer served tip {indexer_height} behind \
             validator tip {validator_height} despite the convergence barrier"
        );
    }
}

/// The serving invariant pepper-sync's `check_tree_size` relies on, verified
/// directly against the wire: per block and per shielded pool, the outputs
/// served in `vtx` account exactly for the growth of the commitment tree
/// size in `chain_metadata`. A consistent sweep alongside a failing wallet
/// scan clears the indexer and points at the scanner or at stale persisted
/// wallet state.
#[tokio::test]
async fn served_outputs_match_chain_metadata_deltas() {
    let (local_net, mut faucet) = scenarios::faucet(
        PoolType::IRONWOOD,
        scenarios::default_test_activation_heights(),
        scenarios::ChainCachePolicy::PerTest,
    )
    .await;

    // A send adds a multi-action ironwood bundle beyond the one-action
    // coinbase blocks.
    let self_address = get_base_address_macro!(faucet, "unified");
    scenarios::send_and_bump(
        &local_net,
        &mut faucet,
        vec![(self_address.as_str(), 100_000, None)],
    )
    .await;

    let mut grpc_client = GrpcIndexer::new(faucet.indexer_uri().expect("client is connected"))
        .await
        .unwrap();
    let tip = grpc_client
        .get_latest_block(DEFAULT_REQUEST_TIMEOUT)
        .await
        .unwrap()
        .height;

    let mut stream = grpc_client
        .get_block_range(
            BlockRange {
                start: Some(BlockId {
                    height: 1,
                    hash: vec![],
                }),
                end: Some(BlockId {
                    height: tip,
                    hash: vec![],
                }),
                // Empty means the legacy default: the server must include all
                // shielded pools. This matches what pepper-sync sends.
                pool_types: vec![],
            },
            DEFAULT_REQUEST_TIMEOUT,
        )
        .await
        .unwrap();

    let ironwood_activation = u64::from(scenarios::IRONWOOD_COINBASE_START_HEIGHT);
    let mut mismatches = Vec::new();
    let mut prev: Option<zingo_netutils::lightwallet_protocol::ChainMetadata> = None;
    let mut ironwood_served_total = 0u64;
    let mut first_nonzero_ironwood_metadata: Option<u64> = None;
    let mut first_served_ironwood_action: Option<u64> = None;

    while let Some(block) = stream.message().await.unwrap() {
        let metadata = block.chain_metadata.unwrap_or_else(|| {
            panic!(
                "indexer serves no chain_metadata at height {}; pepper-sync \
                 cannot verify tree sizes against this server",
                block.height
            )
        });

        let sapling_served = u64::from(block::shielded_output_count(&block, Sapling));
        let orchard_served = u64::from(block::shielded_output_count(&block, Orchard));
        let ironwood_served = u64::from(block::shielded_output_count(&block, Ironwood));

        if metadata.ironwood_commitment_tree_size > 0 {
            first_nonzero_ironwood_metadata.get_or_insert(block.height);
        }
        if ironwood_served > 0 {
            first_served_ironwood_action.get_or_insert(block.height);
        }
        ironwood_served_total += ironwood_served;

        if let Some(prev) = prev {
            for (pool, served, size, prev_size) in [
                (
                    "sapling",
                    sapling_served,
                    metadata.sapling_commitment_tree_size,
                    prev.sapling_commitment_tree_size,
                ),
                (
                    "orchard",
                    orchard_served,
                    metadata.orchard_commitment_tree_size,
                    prev.orchard_commitment_tree_size,
                ),
                (
                    "ironwood",
                    ironwood_served,
                    metadata.ironwood_commitment_tree_size,
                    prev.ironwood_commitment_tree_size,
                ),
            ] {
                let metadata_delta = i64::from(size) - i64::from(prev_size);
                if metadata_delta != served as i64 {
                    mismatches.push(format!(
                        "height {}: {pool} metadata delta {metadata_delta} but \
                         {served} outputs served in vtx",
                        block.height
                    ));
                }
            }
        }
        prev = Some(metadata);
    }

    assert!(
        mismatches.is_empty(),
        "indexer served blocks where chain_metadata growth does not match the \
         outputs present in vtx; the serving side drops outputs or misreports \
         tree sizes:\n{}",
        mismatches.join("\n"),
    );

    // The sweep must actually exercise ironwood serving, otherwise the delta
    // checks above pass vacuously on an indexer that serves neither ironwood
    // actions nor ironwood metadata.
    assert_eq!(
        first_nonzero_ironwood_metadata,
        Some(ironwood_activation),
        "ironwood metadata should first appear at the activation-height \
         coinbase block"
    );
    assert_eq!(
        first_served_ironwood_action,
        Some(ironwood_activation),
        "the activation-height coinbase block should serve an ironwood action"
    );
    // At least one coinbase action per post-activation block, plus the
    // multi-action send bundle.
    assert!(
        ironwood_served_total > tip - ironwood_activation + 1,
        "expected coinbase plus send-bundle ironwood actions, served only \
         {ironwood_served_total} across blocks [{ironwood_activation}, {tip}]"
    );
}

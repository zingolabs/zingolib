use std::{num::NonZeroU32, time::Duration};

use incrementalmerkletree::Position;
use pepper_sync::sync::ScanPriority;
use pepper_sync::test_support::block;
use pepper_sync::wallet::ShardTrees;
use shardtree::store::ShardStore;
use zcash_local_net::validator::Validator;
use zcash_protocol::PoolType;
use zcash_protocol::ShieldedPool::{Ironwood, Orchard, Sapling};
use zcash_protocol::consensus::BlockHeight;
use zingo_netutils::lightwallet_protocol::{BlockId, BlockRange, GetSubtreeRootsArg};
use zingo_netutils::{GrpcIndexer, Indexer};
use zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;
use zingolib::config::{ChainType, ClientConfig, WalletConfig};
use zingolib::data::PollReport;
use zingolib::lightclient::DEFAULT_REQUEST_TIMEOUT;
use zingolib::testutils::default_test_wallet_settings;
use zingolib::testutils::lightclient::from_inputs::quick_send;
use zingolib::testutils::paths::get_cargo_manifest_dir;
use zingolib::testutils::tempfile::TempDir;
use zingolib::{
    config::construct_indexer_uri,
    get_base_address_macro,
    lightclient::LightClient,
    testutils::lightclient::from_inputs::{self},
};

/// The mainnet indexer these ignored, hand-run sync tests dial explicitly.
const SYNC_TEST_INDEXER: &str = "https://zec.rocks:443";
use zingolib_testutils::scenarios::{
    self, IndexerConvergence, increase_height_and_wait_for_client,
};

#[ignore = "temporary mainnet test for sync development"]
#[tokio::test]
async fn sync_mainnet_test() {
    zingolib::ensure_default_crypto_provider();
    tracing_subscriber::fmt().init();

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
            birthday: 1_500_000,
            wallet_settings: default_test_wallet_settings(),
        })
        .build()
        .unwrap();
    let mut lightclient = LightClient::new(config, true).await.unwrap();

    lightclient.sync().await.unwrap();
    let mut interval = tokio::time::interval(zingo_netutils::time::test::SETTLE_POLL_INTERVAL);
    loop {
        interval.tick().await;
        {
            let wallet = lightclient.wallet().read().await;
            tracing::info!("{}", json::JsonValue::from(lightclient.sync_status()));
            tracing::info!("WALLET DEBUG:");
            tracing::info!("uas: {}", wallet.unified_addresses().len());
            tracing::info!("taddrs: {}", wallet.transparent_addresses().len());
            tracing::info!("blocks: {}", wallet.wallet_blocks.len());
            tracing::info!("txs: {}", wallet.wallet_transactions.len());
            tracing::info!("nullifiers o: {}", wallet.nullifier_map.orchard.len());
            tracing::info!("nullifiers s: {}", wallet.nullifier_map.sapling.len());
            tracing::info!("outpoints: {}", wallet.outpoint_map.len());
        }
        lightclient.flush().await.unwrap();
    }

    // let wallet = lightclient.wallet.read().await;
    // dbg!(&wallet.wallet_blocks);
    // dbg!(&wallet.nullifier_map);
    // dbg!(&wallet.sync_state);
}

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
    while !(lightclient
        .wallet()
        .read()
        .await
        .sync_state
        .scan_ranges()
        .iter()
        .any(|range| range.priority() == ScanPriority::Scanning)
        || matches!(lightclient.poll_sync(), PollReport::Ready(_)))
    {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let _ = lightclient.stop_sync();
    lightclient.await_sync().await.unwrap();

    {
        let shard_trees = &mut lightclient.wallet().write().await.shard_trees;

        dbg!("1");
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

    lightclient.sync().await.unwrap();
    while !(lightclient
        .wallet()
        .read()
        .await
        .sync_state
        .scan_ranges()
        .iter()
        .any(|range| range.priority() == ScanPriority::Scanning)
        || matches!(lightclient.poll_sync(), PollReport::Ready(_)))
    {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let _ = lightclient.stop_sync();
    let _ = lightclient.await_sync().await;

    {
        let shard_trees = &mut lightclient.wallet().write().await.shard_trees;

        dbg!("2");
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

/// The raw blocks artifact for `store_all_checkpoints_in_verification_window`,
/// under the gitignored `chain_caches/` root (mirroring the framework's
/// `chain_caches/<binary>/<test>/` keying). The artifact is a
/// hand-managed `LoadRaw` cache rather than a `PerTest` one because the
/// chain's content is wallet sends, which lie past the send boundary
/// that `PerTest` snapshots at.
fn checkpoint_window_blocks_path() -> std::path::PathBuf {
    get_cargo_manifest_dir()
        .parent()
        .expect("libtonode-tests sits directly under the repo root")
        .join("chain_caches/sync/store_all_checkpoints_in_verification_window/raw.blocks")
}

/// Build the ~112-block send-dense chain the checkpoint-window assertion
/// replays, and export it to [`checkpoint_window_blocks_path`]. Runs on
/// the first execution per machine and under
/// [`zingolib_testutils::chain_cache::REGENERATE_ENV`]. Every other run
/// replays the exported artifact.
async fn build_checkpoint_window_chain() {
    // ChainCachePolicy::Disabled, not PerTest: under PerTest the inner
    // scenario would claim this test's chain_caches/<binary>/<test>/
    // directory for its own mined-setup cache, colliding with the raw
    // artifact this builder exports there, and a mined-setup replay
    // would not shorten the build anyway, since the expensive part is
    // the post-boundary sends.
    let (local_net, mut faucet, recipient) = scenarios::faucet_recipient(
        zcash_protocol::PoolType::ORCHARD,
        scenarios::default_test_activation_heights(),
        scenarios::ChainCachePolicy::Disabled,
    )
    .await;

    let recipient_orchard_addr = get_base_address_macro!(recipient, "unified");
    let recipient_sapling_addr = get_base_address_macro!(recipient, "sapling");

    // One dense cycle: four blocks carrying orchard and sapling
    // commitments (and one empty block), mutating both trees.
    macro_rules! dense_cycle {
        () => {
            quick_send(&mut faucet, vec![(&recipient_orchard_addr, 10_000, None)])
                .await
                .unwrap();
            increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
                .await
                .unwrap();

            quick_send(&mut faucet, vec![(&recipient_sapling_addr, 10_000, None)])
                .await
                .unwrap();
            increase_height_and_wait_for_client(&local_net, &mut faucet, 1)
                .await
                .unwrap();

            quick_send(&mut faucet, vec![(&recipient_orchard_addr, 10_000, None)])
                .await
                .unwrap();
            quick_send(&mut faucet, vec![(&recipient_sapling_addr, 10_000, None)])
                .await
                .unwrap();
            increase_height_and_wait_for_client(&local_net, &mut faucet, 2)
                .await
                .unwrap();
        };
    }

    // Dense head, bulk-mined empty middle, dense tail. Proved sends are
    // ~95% of the build's wall clock, and checkpoint presence is
    // per-scanned-block regardless of commitments (adjudicated
    // empirically against the fully dense 27-cycle chain, 2026-07-08),
    // so the dense regions exist to bracket the verification window
    // with genuinely mutating trees, not to fill it: the head cycles
    // straddle the window's start (the pruning boundary crosses a
    // mutated region) and the tail cycles sit at the tip. The empty
    // middle is itself coverage: rewind targets between mutations must
    // checkpoint too.
    for _ in 0..3 {
        dense_cycle!();
    }
    increase_height_and_wait_for_client(&local_net, &mut faucet, 86)
        .await
        .unwrap();
    for _ in 0..3 {
        dense_cycle!();
    }

    zingolib_testutils::chain_cache::export_raw(&local_net, &checkpoint_window_blocks_path()).await;
}

#[tokio::test]
async fn store_all_checkpoints_in_verification_window() {
    let regenerate = std::env::var_os(zingolib_testutils::chain_cache::REGENERATE_ENV)
        .is_some_and(|v| !v.is_empty() && v != *"0");
    if regenerate || !checkpoint_window_blocks_path().exists() {
        build_checkpoint_window_chain().await;
    }
    let (local_net, lightclient) = scenarios::unfunded_client(
        scenarios::default_test_activation_heights(),
        scenarios::ChainCachePolicy::LoadRaw(checkpoint_window_blocks_path()),
    )
    .await;

    // The verification window is anchored to the actual replayed tip,
    // not a hardcoded height: the scenario prelude (launch block,
    // shielded-funds offload) mines blocks before the send cycles, so
    // the chain is longer than the cycles alone and heights below
    // tip − MAX_REORG_ALLOWANCE are legitimately checkpoint-free
    // (pruned; that pruning having run is part of what this exercises).
    let tip = local_net.validator().get_chain_height().await;
    // Retention observed: the last MAX_REORG_ALLOWANCE checkpoints
    // INCLUDING the tip (tip−99..=tip). Whether the finalized fork
    // point at tip−100 also needs its checkpoint (a maximal truncation
    // lands there) is an open question for the pepper-sync owners; if
    // the answer is yes, extend this window down by one and the test
    // correctly goes red until retention grows.
    let window_start = tip - pepper_sync::sync::MAX_REORG_ALLOWANCE + 1;
    for height in window_start..=tip {
        assert!(
            lightclient
                .wallet()
                .read()
                .await
                .shard_trees
                .sapling
                .store()
                .get_checkpoint(&BlockHeight::from_u32(height))
                .unwrap()
                .is_some(),
            "missing sapling checkpoint at height {height}"
        );
        assert!(
            lightclient
                .wallet()
                .read()
                .await
                .shard_trees
                .orchard
                .store()
                .get_checkpoint(&BlockHeight::from_u32(height))
                .unwrap()
                .is_some(),
            "missing orchard checkpoint at height {height}"
        );
        assert!(
            lightclient
                .wallet()
                .read()
                .await
                .shard_trees
                .ironwood
                .store()
                .get_checkpoint(&BlockHeight::from_u32(height))
                .unwrap()
                .is_some(),
            "missing ironwood checkpoint at height {height}"
        );
    }
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
    while !(lightclient
        .wallet()
        .read()
        .await
        .sync_state
        .scan_ranges()
        .iter()
        .any(|range| range.priority() == ScanPriority::Scanning)
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

/// A wallet holding blocks that were scanned without tracking a pool must
/// still end up with the notes those blocks contain. Nothing in such a
/// wallet asks for them: the ranges are recorded as fully scanned, so the
/// notes remain lost and the balance silently understates what the wallet
/// owns.
///
/// The wallet's own recorded commitment tree for the pool is what gives it
/// away, because it cannot account for the tree the chain reports. Nothing
/// about that reasoning is particular to one pool, one activation height or
/// one network, so it is exercised for each pool a wallet can be funded in.
async fn notes_in_untracked_history_are_recovered(pool: PoolType) {
    let shielded_pool = match pool {
        PoolType::ORCHARD => zcash_protocol::ShieldedPool::Orchard,
        PoolType::IRONWOOD => zcash_protocol::ShieldedPool::Ironwood,
        other => panic!("{other} is not a pool a recipient can be funded in"),
    };
    let balance = async |client: &LightClient| -> u64 {
        let balance = client
            .account_balance(zip32::AccountId::ZERO)
            .await
            .expect("a synced client reports a balance");
        match shielded_pool {
            zcash_protocol::ShieldedPool::Orchard => balance.confirmed_orchard_balance,
            zcash_protocol::ShieldedPool::Ironwood => balance.confirmed_ironwood_balance,
            zcash_protocol::ShieldedPool::Sapling => balance.confirmed_sapling_balance,
        }
        .map(zcash_protocol::value::Zatoshis::into_u64)
        .unwrap_or(0)
    };

    let fixture = scenarios::default_test_activation_heights();
    let activation_heights = zingolib::ActivationHeights::builder()
        .set_overwinter(fixture.overwinter())
        .set_sapling(fixture.sapling())
        .set_blossom(fixture.blossom())
        .set_heartwood(fixture.heartwood())
        .set_canopy(fixture.canopy())
        .set_nu5(fixture.nu5())
        .set_nu6(fixture.nu6())
        .set_nu6_1(fixture.nu6_1())
        .set_nu6_2(fixture.nu6_2())
        .set_nu6_3(match shielded_pool {
            zcash_protocol::ShieldedPool::Ironwood => fixture.nu6_3(),
            _ => None,
        })
        .set_nu7(None)
        .build();

    let (local_net, mut faucet, mut recipient) = scenarios::faucet_recipient(
        pool,
        activation_heights,
        scenarios::ChainCachePolicy::PerTest,
    )
    .await;

    let recipient_address = get_base_address_macro!(recipient, "unified");
    scenarios::send_and_bump(
        &local_net,
        &mut faucet,
        vec![(recipient_address.as_str(), 100_000, None)],
    )
    .await;
    scenarios::sync_client_to_validator_tip(&local_net, &mut recipient).await;

    let funded_balance = balance(&recipient).await;
    assert!(
        funded_balance > 0,
        "the recipient must hold a {pool} note before its history is stripped"
    );

    // Reduce the wallet to what a build that never tracked the pool would
    // have left: the very same scanned ranges, none of the pool's data.
    {
        let wallet = recipient.wallet();
        let mut wallet = wallet.write().await;
        pepper_sync::wallet::strip_pool_tracking_for_test(&mut *wallet, shielded_pool)
            .expect("stripping a local wallet is infallible");
    }
    assert_eq!(
        balance(&recipient).await,
        0,
        "stripping must actually remove the note that recovery is then judged by"
    );

    // Mining gives the wallet a block whose metadata it must account for.
    // The barrier is held here rather than by a syncing helper, so the
    // sessions below are counted exactly.
    let target = scenarios::generate_n_blocks_return_new_height(&local_net, 2).await;
    local_net.converge(target).await;

    // The first session finds the pool's recorded tree wanting and reopens
    // its history, ending there rather than reporting a sync that would have
    // left the notes lost.
    let reopened = recipient.sync_and_await().await;
    assert!(
        reopened.is_err(),
        "a wallet whose {pool} history cannot account for the chain must not \
         report a successful sync: {reopened:?}"
    );

    // The second session rescans the reopened history and recovers the note.
    scenarios::sync_client_to_validator_tip(&local_net, &mut recipient).await;

    assert_eq!(
        balance(&recipient).await,
        funded_balance,
        "the {pool} note was left lost in history that was scanned without \
         {pool} tracking",
    );
}

#[tokio::test]
async fn ironwood_notes_in_untracked_history_are_recovered() {
    notes_in_untracked_history_are_recovered(PoolType::IRONWOOD).await;
}

#[tokio::test]
async fn orchard_notes_in_untracked_history_are_recovered() {
    notes_in_untracked_history_are_recovered(PoolType::ORCHARD).await;
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

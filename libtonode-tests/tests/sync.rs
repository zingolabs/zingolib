use std::{num::NonZeroU32, time::Duration};

use incrementalmerkletree::Position;
use pepper_sync::sync::ScanPriority;
use pepper_sync::wallet::ShardTrees;
use shardtree::store::ShardStore;
use zcash_local_net::validator::Validator;
use zcash_protocol::consensus::BlockHeight;
use zingo_netutils::lightwallet_protocol::GetSubtreeRootsArg;
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
    config::{DEFAULT_INDEXER_URI, construct_lightwalletd_uri},
    get_base_address_macro,
    lightclient::LightClient,
    testutils::lightclient::from_inputs::{self},
};
use zingolib_testutils::scenarios::{self, increase_height_and_wait_for_client};

#[ignore = "temporary mainnet test for sync development"]
#[tokio::test]
async fn sync_mainnet_test() {
    zingolib::ensure_default_crypto_provider();
    tracing_subscriber::fmt().init();

    let uri = construct_lightwalletd_uri(Some(DEFAULT_INDEXER_URI.to_string())).unwrap();
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
        .build();
    let mut lightclient = LightClient::new(config, true).await.unwrap();

    lightclient.sync().await.unwrap();
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    loop {
        interval.tick().await;
        {
            let wallet = lightclient.wallet().read().await;
            tracing::info!(
                "{}",
                json::JsonValue::from(pepper_sync::sync_status(&*wallet).await.unwrap())
            );
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

    let uri = construct_lightwalletd_uri(Some(DEFAULT_INDEXER_URI.to_string())).unwrap();
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
        .build();
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
    let _ = lightclient.await_sync().await;

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
/// [`zingolib_testutils::chain_cache::REGENERATE_ENV`]; every other run
/// replays the exported artifact.
async fn build_checkpoint_window_chain() {
    // ChainCachePolicy::Disabled, not PerTest: under PerTest the inner
    // scenario would claim this test's chain_caches/<binary>/<test>/
    // directory for its own mined-setup cache — colliding with the raw
    // artifact this builder exports there — and a mined-setup replay
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
    // empirically against the fully dense 27-cycle chain, 2026-07-08) —
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
/// container runs — twice, the wallet at exactly 1096 both times, under
/// `makers container-test` (podman on the originating machine) — while
/// host runs of the same commit passed. "Container-only" records where
/// it happened, not a mechanism: in the same failing container process
/// a parallel fresh connection to the same URI saw all 1128 roots, so
/// the healthy backend was reachable from inside the container, and a
/// backend's state is client-independent. What pinned the wallet's
/// channel to the stuck backend in exactly the container runs is
/// unresolved — logging the resolved backend identity (the issue's open
/// question) would make a recurrence attributable. Note the
/// stream-resume defense of 9d40495b9 converts mid-flight cuts into
/// completions but cannot conjure shards a stuck backend does not have,
/// so present-day non-reproduction indicates a healthy backend pool
/// (a day-later container run pulled 1128 cleanly), not a client fix.
///
/// Prints, for three fresh connections, how many sapling subtree roots the
/// stream yields and the chain height at which it ends — then the count
/// pepper-sync's own fetch path lands in the wallet. If a short stream's last
/// completing height is months old, the connection reached a lagging backend
/// behind the load balancer; if counts differ between connections to a
/// current backend, the stream is being cut mid-flight.
///
/// Run in the environment under investigation with output visible, e.g.:
///   makers test -p libtonode-tests \
///     -E 'test(diagnose_subtree_root_stream)' --run-ignored all --no-capture
#[ignore = "diagnostic: run manually against live mainnet"]
#[tokio::test]
async fn diagnose_subtree_root_stream() {
    let uri = construct_lightwalletd_uri(Some(DEFAULT_INDEXER_URI.to_string())).unwrap();

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
        .build();
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

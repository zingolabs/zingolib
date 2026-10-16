use std::{num::NonZeroU32, time::Duration};

use shardtree::store::ShardStore;
use zcash_local_net::validator::Validator;
use zingo_netutils::lightwallet_protocol::GetSubtreeRootsArg;
use zingo_netutils::{GrpcIndexer, Indexer};
use zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;
use zingolib::config::{ChainType, ClientConfig, WalletConfig};
use zingolib::data::PollReport;
use zingolib::lightclient::DEFAULT_REQUEST_TIMEOUT;
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

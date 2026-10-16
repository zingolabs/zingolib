use zcash_local_net::validator::Validator;
use zingo_netutils::{GrpcIndexer, Indexer};
use zingolib::lightclient::DEFAULT_REQUEST_TIMEOUT;

use zingolib_testutils::scenarios;

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

//! The pipeline-observability evidence test.
//!
//! HYPOTHESIS (warm-suite adjudicated, 2026-07-08): a regtest
//! Validator's chain mutates only via its RPC surface. If the chain
//! moves while the taps and the RPC ledger are silent, the hypothesis
//! is DISPROVEN and something other than RPC mutates regtest chains.
//! The launch-contract falsification sentinels that grew out of the
//! same investigation live in `tests/sentinels.rs` behind the
//! non-default `sentinels` feature; this test stays in the default
//! suite because it exercises this repository's own scenario, replay,
//! and isolation machinery under real suite concurrency.

use std::time::Duration;

use zcash_protocol::PoolType;

use zingolib_testutils::scenarios;
use zingolib_testutils::validator_rpc;

/// The strongest available evidence that a chain mutation did not come
/// from the test itself: after setup completes, this test's code
/// contains no chain-mutating call at all — the window between the two
/// tip samples is RPC-silent *by construction*, and the ledger assert
/// proves the construction held at runtime. Uses the transparent-pool
/// `faucet_recipient` shape: its chains are byte-identical across
/// tests, making it both the likeliest victim and the likeliest
/// attacker of the cross-wire hypothesis, and the deterministic
/// reproducer of the warm-suite failures when run alongside them.
#[tokio::test]
async fn chain_mutates_only_via_owned_rpc() {
    let (local_net, _faucet, _recipient) = scenarios::faucet_recipient(
        PoolType::Transparent,
        scenarios::default_test_activation_heights(),
        scenarios::ChainCachePolicy::PerTest,
    )
    .await;

    let tip_before = local_net
        .zebrad_watch()
        .last()
        .expect("the watch observed setup complete");
    let window_opened_at = validator_rpc::ledger_snapshot().len();

    // The bound-justified window: no statement between the two samples
    // issues a validator RPC. The only ledger entries the window may
    // accumulate are the state watches' read-only polls.
    tokio::time::sleep(Duration::from_secs(10)).await;

    let writes_in_window: Vec<_> = validator_rpc::ledger_snapshot()[window_opened_at..]
        .iter()
        .filter(|entry| validator_rpc::is_write_method(&entry.method))
        .cloned()
        .collect();
    assert!(
        writes_in_window.is_empty(),
        "the RPC-silent window was not silent — this test's own harness issued writes: \
         {writes_in_window:?}"
    );

    let tip_after = local_net
        .zebrad_watch()
        .last()
        .expect("the watch is still observing");
    assert_eq!(
        tip_before.fingerprint,
        tip_after.fingerprint,
        "chain mutated during an RPC-silent window — foreign traffic, or a non-RPC \
         mutation channel (hypothesis disproven)!\n\
         zebrad timeline:\n{}\n\
         zainod timeline:\n{}\n\
         harness->zebrad tap:\n{}\n\
         wallet->zainod tap:\n{}\n\
         zainod->zebrad tap:\n{}",
        local_net.zebrad_watch().render(),
        local_net.zainod_watch().render(),
        local_net.rpc_tap().render(),
        local_net.indexer_tap().render(),
        local_net.validator_tap().render(),
    );
}

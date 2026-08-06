//! Sentinel tests: standing falsification attempts against the
//! environment contracts the suite's machinery assumes.
//!
//! These pin behaviors of zebrad and of `zcash_local_net`'s launch
//! path, not this repository's code: no zls change can fail them, and
//! they only fire when the environment moves (an infrastructure pin
//! bump, a container-image bump, an activation-height change). They
//! are therefore feature-gated off the default suite. Run them with
//! `--features sentinels` (or the `extra-credit-tests` aggregate)
//! whenever one of those inputs changes.
//!
//! Each test names the claim its failure disproves. The claims and
//! their history live in `docs/adr/0003-test-owned-chain-caches.md`
//! and the Launch block entry of `CONTEXT.md`.
#![cfg(feature = "sentinels")]

use std::path::PathBuf;

use zcash_local_net::MinerPool;
use zcash_local_net::process::Process;
use zcash_local_net::validator::{Validator, ValidatorConfig};

use zingolib_testutils::scenarios::{self, network_combo::DefaultValidator};
use zingolib_testutils::validator_rpc;

/// Launch a bare transparent-pool Validator (no Indexer, no scenario
/// harness) for the launch-block falsification tests. Bare `Zebrad`
/// has no `Drop`-stop, so callers must `stop()` it themselves.
async fn launch_bare_transparent_zebrad(chain_cache: Option<PathBuf>) -> DefaultValidator {
    zingolib::ensure_default_crypto_provider();
    let mut config = <DefaultValidator as Process>::Config::default();
    config.set_test_parameters(
        MinerPool::Transparent,
        scenarios::default_net_activation_heights(),
        chain_cache,
    );
    <DefaultValidator as Process>::launch(config)
        .await
        .expect("bare zebrad must launch")
}

/// H1 falsification attempt: the ONLY launch-time chain mutation is the
/// single `generate_blocks(1)` inside `Zebrad::launch`, and nothing
/// mines unbidden afterward. Predictions under H1: a bare launch
/// returns at exactly height 1, this test's own ledger holds no write
/// calls, and the tip is byte-stable over an idle window. A rising or
/// changing tip disproves H1 (something mines on its own). Height ≠ 1
/// at launch-return disproves the launch-mine account itself.
#[tokio::test]
async fn launch_mines_exactly_one_block_and_nothing_else_mines() {
    let mut zebrad = launch_bare_transparent_zebrad(None).await;
    let rpc_port = zebrad.rpc_listen_port();

    let (height, tip) = validator_rpc::try_get_chain_info(rpc_port)
        .await
        .expect("freshly launched zebrad must answer");
    assert_eq!(
        height, 1,
        "launch returned at height {height}, not 1, so the launch-mine account is wrong"
    );
    let writes: Vec<_> = validator_rpc::ledger_snapshot()
        .iter()
        .filter(|entry| validator_rpc::is_write_method(&entry.method))
        .cloned()
        .collect();
    assert!(
        writes.is_empty(),
        "this test issued writes itself, so it cannot attribute block 1: {writes:?}"
    );

    tokio::time::sleep(zingo_netutils::time::test::IDLE_OBSERVATION_WINDOW).await;
    let (idle_height, idle_tip) = validator_rpc::try_get_chain_info(rpc_port)
        .await
        .expect("idle zebrad must still answer");
    assert_eq!(
        (idle_height, &idle_tip),
        (1, &tip),
        "the chain moved during an idle window with no client attached. \
         H1 disproven: something mines without being asked"
    );

    zebrad.stop();
}

/// H2 falsification attempt: transparent-pool launch blocks are
/// byte-deterministic, the mechanism behind the warm-run "duplicate"
/// verdicts, whose inferred deterministic header timestamp is the
/// least-verified link in the launch-block account. Two launches
/// seconds apart must produce the same block-1 hash (the hash covers
/// the header and, through the merkle root, the whole block).
/// Inequality disproves H2 and reopens the duplicate explanation.
#[tokio::test]
async fn transparent_launch_block_is_byte_deterministic() {
    let mut first = launch_bare_transparent_zebrad(None).await;
    let (_, first_tip) = validator_rpc::try_get_chain_info(first.rpc_listen_port())
        .await
        .expect("first zebrad must answer");

    let mut second = launch_bare_transparent_zebrad(None).await;
    let (_, second_tip) = validator_rpc::try_get_chain_info(second.rpc_listen_port())
        .await
        .expect("second zebrad must answer");

    assert_eq!(
        first_tip, second_tip,
        "two transparent launch blocks differ, so H2 (byte-determinism) is disproven. \
         The warm-run duplicate verdicts need a different explanation"
    );

    first.stop();
    second.stop();
}

/// H1 counterfactual: the launch-mine is skipped when the config
/// carries a chain cache (`zebrad.rs` gates the generate on
/// `chain_cache.is_none()`), and a state-dir cache of a 1-block chain
/// holds essentially genesis (blocks within the finalization depth,
/// zebra's `MAX_BLOCK_REORG_HEIGHT`, mirrored here by
/// `pepper_sync::sync::MAX_REORG_ALLOWANCE`, live only in memory).
/// Launching from such a cache therefore removes the hypothesized
/// cause. H1 predicts the effect disappears too: height 0, sustained.
/// Height 1 immediately would mean short-chain state persists after
/// all (revising the finalization claim, not H1). Height rising during
/// the idle window would disprove H1 outright.
#[tokio::test]
async fn suppressed_launch_generate_leaves_genesis() {
    let mut donor = launch_bare_transparent_zebrad(None).await;
    let cache_dir = std::env::temp_dir().join(format!(
        "suppressed_launch_generate_cache_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&cache_dir);
    let output = donor.cache_chain(cache_dir.clone()).await;
    assert!(output.status.success(), "state-dir copy failed");

    let mut subject = launch_bare_transparent_zebrad(Some(cache_dir.clone())).await;
    let rpc_port = subject.rpc_listen_port();
    let (height, tip) = validator_rpc::try_get_chain_info(rpc_port)
        .await
        .expect("cache-loaded zebrad must answer");
    assert_eq!(
        height, 0,
        "cause suppressed but chain at height {height} (tip {tip}): either short-chain \
         state persists on disk after all, or something other than the launch generate mines"
    );

    tokio::time::sleep(zingo_netutils::time::test::IDLE_OBSERVATION_WINDOW).await;
    let (idle_height, idle_tip) = validator_rpc::try_get_chain_info(rpc_port)
        .await
        .expect("idle cache-loaded zebrad must still answer");
    assert_eq!(
        (idle_height, &idle_tip),
        (0, &tip),
        "the chain grew with the launch generate suppressed and no client attached. \
         H1 disproven: the launch generate is not the only unprompted mutation"
    );

    subject.stop();
    let _ = std::fs::remove_dir_all(&cache_dir);
}

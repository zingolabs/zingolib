//! Root-cause discrimination suite for zebra's differential rejection of
//! tip-anchored spends.
//!
//! # Phenomenon
//!
//! The pool_matrix orchard-source rows fail in zebra's mempool with
//! "could not validate orchard proof ... will be rejected from the mempool
//! until the next chain tip block", during the funding send (orchard spends
//! with an orchard output). The sapling-source rows — whose funding sends
//! make the SAME orchard spends but with a sapling output — pass. The
//! normalize_shielded_faucet_balance offload historically hit the same rejection when
//! it spent the tip block's coinbase note (fixed by sync-then-mine
//! separation in f39cee419).
//!
//! # Hypotheses
//!
//! - H-NOTE: the trigger is spending a note BORN IN THE TIP BLOCK,
//!   regardless of output pool. The sapling rows escaped because the
//!   faucet's note selection happened to pick older notes for those
//!   funding amounts.
//! - H-ANCHOR: the trigger is the orchard ANCHOR referencing the tip
//!   block's post-state root; note age is irrelevant.
//! - H-OUTPUT: the trigger correlates with the presence of an orchard
//!   OUTPUT (bundle shape), not with spends or anchors.
//! - H-SELECT: there is no wallet/zebra asymmetry at all; the matrix rows
//!   differ only because fee-table differences in funding amounts flip
//!   note selection between old and tip notes.
//!
//! # Design
//!
//! Every test gives the sending wallet EXACTLY ONE spendable orchard note,
//! which eliminates note selection and thereby kills H-SELECT as a
//! confound in these cells. The wallet is synced to the validator tip in
//! all four cells, so the anchor height is held constant (at the tip);
//! note depth and output pool vary independently:
//!
//! | test                  | note depth | output pool | H-NOTE says | H-ANCHOR says | H-OUTPUT says |
//! |-----------------------|------------|-------------|-------------|---------------|---------------|
//! | tip_note_to_orchard   | 0 (tip)    | orchard     | rejected    | rejected      | rejected      |
//! | tip_note_to_sapling   | 0 (tip)    | sapling     | rejected    | rejected      | ACCEPTED      |
//! | aged_note_to_orchard  | 3          | orchard     | ACCEPTED    | rejected      | rejected      |
//! | aged_note_to_sapling  | 3          | sapling     | ACCEPTED    | ACCEPTED      | ACCEPTED      |
//!
//! The four verdicts read as a column of this table and select the
//! hypothesis (or falsify all three, which is itself progress). The
//! encoded assertions below state the H-NOTE column — the leading
//! hypothesis given the f39cee419 prior — so a different truth table
//! surfaces as test failures whose messages print the observed cell.
//!
//! # Round one verdict (2026-07-06, host stack)
//!
//! All four cells were ACCEPTED, falsifying H-NOTE, H-ANCHOR, and
//! H-OUTPUT as stated: on a tall chain (funded-faucet scenario, height
//! ~106, all activations long past), a non-miner wallet spending a
//! non-coinbase orchard note is accepted at every depth and output pool,
//! tip-anchored or not. The factors round one did NOT reproduce from the
//! failing pool_matrix conditions are the round-two hypothesis space:
//!
//! - H-BOUNDARY: the send is rejected when built near the height-5
//!   NU6.1/6.2 co-activation (the matrix envs sit at height ~3-4).
//! - H-COINBASE: spending a YOUNG ORCHARD COINBASE note (the miner
//!   faucet's only funds on a short chain) is what zebra rejects, and
//!   the orchard-proof message is how it surfaces.
//!
//! # Round two design
//!
//! The matrix_* cells below rebuild the exact chain-generics environment
//! (short chain, miner faucet whose only notes are young orchard
//! coinbases) and vary one factor per cell:
//!
//! | test                              | extra blocks | output pool | H-BOUNDARY says | H-COINBASE says |
//! |-----------------------------------|--------------|-------------|-----------------|-----------------|
//! | matrix_young_coinbase_to_orchard  | 0            | orchard     | rejected        | rejected        |
//! | matrix_young_coinbase_to_sapling  | 0            | sapling     | rejected (footnote one) | rejected        |
//! | matrix_aged_coinbase_to_orchard   | 10           | orchard     | ACCEPTED        | rejected (footnote two) |
//!
//! Footnote one: H-BOUNDARY predicts rejection for BOTH output pools near
//! the boundary; an accept in the sapling cell while the orchard cell
//! rejects reproduces the differential under identical amounts and note
//! selection, eliminating H-SELECT for good.
//!
//! Footnote two: ten extra blocks leave the coinbase notes still far
//! younger than transparent maturity; if youth is the trigger that cell
//! still rejects, while H-BOUNDARY says the boundary is now five blocks
//! behind and the send goes through.

//! # Round four (2026-07-16): classification by the direct channel
//!
//! zainod 0.6.0 masks zebra's rejection text as opaque Internal errors
//! (zaino#1404), so the indexer-path error STRING is no longer a
//! trustworthy classifier. Every cell now classifies its verdict by
//! the validator's own judgement of the wallet's retained bytes,
//! via [`zingolib_testutils::attribution::attribute_send_failure`] —
//! the dual-channel/dual-time probe round three prototyped, extracted
//! into a shared measure. Failures print the full attribution, so a
//! red cell names the guilty layer (wallet builder, indexer
//! transport, or validator verdict) in its own failure message.

//! # Round five verdict (2026-07-16, zebrad 6.0.0)
//!
//! The zebrad 6.0.0-rc.0 → 6.0.0 bump removed the phenomenon: the
//! boundary-adjacent orchard-output sends are ACCEPTED, observed
//! twice deterministically in both the attribution environment and
//! the matrix cell. The mechanism was inside zebra rc.0's mempool
//! admission (reworked in 6.0.0); H-WALLET-CONTEXT is revised and the
//! wallet builder exonerated. Every cell now pins acceptance, and the
//! suite's residual value is as a regression sentinel: a relapse
//! fails with the full attribution in the failure message. Separately,
//! `faucet_funded_recipient` now funds via the ironwood pool, so the
//! run_cell cells locate the funding note in either shielded pool.

use zingolib::get_base_address_macro;
use zingolib::lightclient::LightClient;
use zingolib::testutils::lightclient::from_inputs;
use zingolib_testutils::attribution;
use zingolib_testutils::scenarios::{self, increase_height_and_wait_for_client};
use zingolib_testutils::setup_metrics::MeteredNet;

/// The classification of one experimental cell.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Accepted,
    TipRejected,
}

/// Blocks of distance the attribution probe mines before its second
/// direct submission — enough to clear the height-5 co-activation
/// from any short-chain environment.
const ATTRIBUTION_DISTANCE_BLOCKS: u32 = 5;

/// Whether a validator rejection message is the tip-rejection class
/// this suite studies (either surface form zebra uses for it).
fn is_tip_rejection(message: &str) -> bool {
    message.contains("until the next chain tip block") || message.contains("orchard proof")
}

/// Classifies one send outcome by the validator's direct verdicts.
/// An error outside the hypothesis space panics with the full
/// attribution, so the failure message names the guilty layer.
async fn classify_send_outcome(
    local_net: &MeteredNet,
    sender: &LightClient,
    result: Result<(), String>,
    observables: &str,
) -> (Verdict, String) {
    match result {
        Ok(()) => (Verdict::Accepted, observables.to_string()),
        Err(indexer_error) => {
            let attribution =
                attribution::attribute_send_failure(local_net, sender, ATTRIBUTION_DISTANCE_BLOCKS)
                    .await;
            let observables = format!("{observables}, attribution=[{attribution}]");
            match attribution.boundary_error() {
                Some(message) if is_tip_rejection(message) => (Verdict::TipRejected, observables),
                _ => panic!(
                    "outcome outside the hypothesis space: \
                     indexer_error={indexer_error} ({observables})"
                ),
            }
        }
    }
}

/// Runs one cell: fund the recipient with a single shielded note (the
/// scenario funds via the newest pool — ironwood since the
/// `PoolType::IRONWOOD` migration of `faucet_funded_recipient`; the
/// cells were originally observed with orchard funding), age it
/// `depth` blocks (the wallet stays synced to the tip, so the anchor
/// is always the tip), then send to the faucet's address in
/// `target_pool` and classify zebra's verdict.
async fn run_cell(depth: u32, target_pool: &str) -> (Verdict, String) {
    let (ref local_net, faucet, mut recipient, _txid) =
        scenarios::faucet_funded_recipient_default(100_000).await;

    if depth > 0 {
        increase_height_and_wait_for_client(local_net, &mut recipient, depth)
            .await
            .unwrap();
    }

    let note_height = recipient
        .transaction_summaries(false)
        .await
        .unwrap()
        .iter()
        .find(|summary| !summary.ironwood_notes.is_empty() || !summary.orchard_notes.is_empty())
        .expect("the recipient holds its funding note")
        .blockheight;
    let target_address = get_base_address_macro!(faucet, target_pool);

    let result = from_inputs::quick_send(&mut recipient, vec![(&target_address, 20_000, None)])
        .await
        .map(|_| ())
        .map_err(|error| error.to_string());
    let observables = format!(
        "note_height={note_height}, depth_argument={depth}, target_pool={target_pool}, result={result:?}"
    );
    classify_send_outcome(local_net, &recipient, result, &observables).await
}

/// Round-one verdict: ACCEPTED (falsifying every hypothesis that keyed
/// on note freshness, anchors, or output pool in this environment). A
/// fresh non-coinbase orchard note on a tall chain spends fine with an
/// orchard output; the assertion pins that observed invariant.
#[tokio::test]
async fn tip_note_to_orchard() {
    let (verdict, observables) = run_cell(0, "unified").await;
    assert_eq!(verdict, Verdict::Accepted, "{observables}");
}

/// Round-one verdict: ACCEPTED, same invariant with a sapling output.
#[tokio::test]
async fn tip_note_to_sapling() {
    let (verdict, observables) = run_cell(0, "sapling").await;
    assert_eq!(verdict, Verdict::Accepted, "{observables}");
}

/// Sorts H-NOTE (accepted) from H-ANCHOR (rejected): the note is three
/// blocks deep, but the wallet is synced to the tip so the anchor still
/// references the tip.
#[tokio::test]
async fn aged_note_to_orchard() {
    let (verdict, observables) = run_cell(3, "unified").await;
    assert_eq!(verdict, Verdict::Accepted, "{observables}");
}

/// Control cell: an aged note with a sapling output; every hypothesis
/// predicts acceptance, so a rejection here falsifies the whole space.
#[tokio::test]
async fn aged_note_to_sapling() {
    let (verdict, observables) = run_cell(3, "sapling").await;
    assert_eq!(verdict, Verdict::Accepted, "{observables}");
}

/// Rebuilds the pool_matrix environment (short chain near the height-5
/// co-activation; miner faucet whose only funds are young orchard
/// coinbase notes) and classifies one faucet send.
async fn run_matrix_cell(extra_blocks: u32, target_pool: &str) -> (Verdict, String) {
    use zingolib::testutils::chain_generics::conduct_chain::ConductChain;

    let mut environment = libtonode_tests::chain_generics::LibtonodeEnvironment::setup().await;
    let mut faucet = environment.create_faucet().await;
    let recipient = environment.create_client().await;
    for _ in 0..extra_blocks {
        environment.increase_chain_height().await;
    }
    faucet.sync_and_await().await.unwrap();

    let target_address = get_base_address_macro!(recipient, target_pool);
    let result = from_inputs::quick_send(&mut faucet, vec![(&target_address, 20_000, None)])
        .await
        .map(|_| ())
        .map_err(|error| error.to_string());
    let observables =
        format!("extra_blocks={extra_blocks}, target_pool={target_pool}, result={result:?}");
    classify_send_outcome(&environment.local_net, &faucet, result, &observables).await
}

/// The in-suite reproduction of the pool_matrix orchard-row failure.
///
/// Round-five verdict (2026-07-16, zebrad 6.0.0): ACCEPTED, observed
/// twice deterministically. The rejection this cell reproduced lived
/// in zebra rc.0's mempool admission and is fixed upstream; the pin
/// flips to acceptance, so a relapse surfaces as a failure whose
/// message carries the full attribution.
#[tokio::test]
async fn matrix_young_coinbase_to_ironwood() {
    let (verdict, observables) = run_matrix_cell(0, "unified").await;
    assert_eq!(verdict, Verdict::Accepted, "{observables}");
}

/// The differential under identical amounts and note selection: same
/// sender, same young coinbase funds, sapling output. The pool_matrix
/// sapling rows say this is accepted; H-COINBASE says rejected.
#[tokio::test]
async fn matrix_young_coinbase_to_sapling() {
    let (verdict, observables) = run_matrix_cell(0, "sapling").await;
    assert_eq!(verdict, Verdict::Accepted, "{observables}");
}

/// Sorts H-BOUNDARY (accepted: the activation is now well behind) from
/// H-COINBASE (rejected: the coinbase notes are still young).
#[tokio::test]
async fn matrix_aged_coinbase_to_orchard() {
    let (verdict, observables) = run_matrix_cell(10, "unified").await;
    assert_eq!(verdict, Verdict::Accepted, "{observables}");
}

/// # Round three: attribution of the boundary rejection
///
/// Round two localized the trigger (orchard-output transactions built
/// adjacent to the height-5 co-activation) but not the culprit: the
/// wallet could be building a context-bad transaction, zainod could be
/// transforming the verdict, or zebra could be wrongly rejecting valid
/// bytes. The wallet retains the built Transaction in its record when
/// transmission fails (status Failed), so the exact rejected bytes are
/// recoverable, and the validator's JSON-RPC port is directly reachable
/// via rpc_listen_port — no indexer in the loop.
///
/// Two predicates on the SAME bytes sort the hypothesis space:
///
/// - Verdict parity NOW: submitting the captured bytes directly to
///   zebra's sendrawtransaction at the boundary must reproduce the
///   rejection. Parity exonerates zainod as a transport (it relayed
///   zebra's verdict faithfully); a divergent verdict implicates it.
/// - Same bytes LATER: after five blocks of distance, resubmit the
///   IDENTICAL bytes. Zebra's own error text ("until the next chain tip
///   block") predicts acceptance; the round-two cure worked with a
///   REBUILT transaction, which left both explanations open. If the
///   identical bytes are accepted, the proof was valid all along and
///   zebra's boundary-time verdict was wrong (the mechanism is inside
///   zebra); if they are still rejected, the wallet built a transaction
///   only valid under post-boundary rules and zebra was right both
///   times (H-WALLET-CONTEXT).
///
/// Round four extracted this probe into
/// [`zingolib_testutils::attribution::attribute_send_failure`] so any
/// suite can attribute a send failure.
///
/// # Round five: the phenomenon is fixed upstream
///
/// Under zebrad 6.0.0 (bumped 2026-07-16) the boundary-adjacent
/// orchard-output send is ACCEPTED — observed twice deterministically
/// in this environment and in the matrix cell. The mechanism was
/// inside zebra rc.0's mempool admission after all, the outcome round
/// three's "same bytes LATER" predicate had named as the
/// mechanism-inside-zebra branch; the wallet builder is exonerated,
/// and the round-three H-WALLET-CONTEXT conviction is revised (the
/// wrong-branch-id rejection of the resubmitted bytes was the
/// expected fate of boundary-built bytes crossing an activation, not
/// evidence against the wallet). This test is therefore now the
/// boundary-ACCEPTANCE sentinel: the send that cost three rounds of
/// investigation must succeed. A relapse fails the assertion with the
/// full attribution in the message, self-diagnosed by the same probe.
#[tokio::test]
async fn boundary_rejection_attribution() {
    use zingolib::config::WalletConfig;
    use zingolib::testutils::default_test_wallet_settings;

    let (local_net, mut client_builder) = scenarios::custom_clients_default().await;
    let mut faucet = client_builder.build_faucet(false).await;
    let recipient = client_builder
        .build_client(
            WalletConfig::MnemonicPhrase {
                mnemonic_phrase: zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED.to_string(),
                no_of_accounts: 1.try_into().unwrap(),
                birthday: 1,
                wallet_settings: default_test_wallet_settings(),
            },
            true,
        )
        .await;
    faucet.sync_and_await().await.unwrap();

    let target_address = get_base_address_macro!(recipient, "unified");
    let result = from_inputs::quick_send(&mut faucet, vec![(&target_address, 20_000, None)])
        .await
        .map(|_| ())
        .map_err(|error| error.to_string());
    let observables = format!("boundary-adjacent orchard-output send, result={result:?}");
    let (verdict, observables) =
        classify_send_outcome(&local_net, &faucet, result, &observables).await;
    assert_eq!(verdict, Verdict::Accepted, "{observables}");
}

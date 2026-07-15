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

use zingolib::get_base_address_macro;
use zingolib::testutils::lightclient::from_inputs;
use zingolib_testutils::scenarios::{self, increase_height_and_wait_for_client};

/// The classification of one experimental cell.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Accepted,
    TipRejected,
}

/// Runs one cell: fund the recipient with a single orchard note, age it
/// `depth` blocks (the wallet stays synced to the tip, so the anchor is
/// always the tip), then send to the faucet's address in `target_pool`
/// and classify zebra's verdict.
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
        .find(|summary| !summary.orchard_notes.is_empty())
        .expect("the recipient holds its funding note")
        .blockheight;
    let target_address = get_base_address_macro!(faucet, target_pool);

    let result = from_inputs::quick_send(&mut recipient, vec![(&target_address, 20_000, None)])
        .await
        .map(|_| ());
    let observables = format!(
        "note_height={note_height}, depth_argument={depth}, target_pool={target_pool}, result={result:?}"
    );
    let verdict = match result {
        Ok(()) => Verdict::Accepted,
        Err(error) if error.to_string().contains("until the next chain tip block") => {
            Verdict::TipRejected
        }
        Err(other) => panic!("outcome outside the hypothesis space: {other} ({observables})"),
    };
    (verdict, observables)
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
        .map(|_| ());
    let observables =
        format!("extra_blocks={extra_blocks}, target_pool={target_pool}, result={result:?}");
    let verdict = match result {
        Ok(()) => Verdict::Accepted,
        Err(error) if error.to_string().contains("until the next chain tip block") => {
            Verdict::TipRejected
        }
        Err(other) => panic!("outcome outside the hypothesis space: {other} ({observables})"),
    };
    (verdict, observables)
}

/// The in-suite reproduction of the pool_matrix orchard-row failure.
#[tokio::test]
async fn matrix_young_coinbase_to_ironwood() {
    let (verdict, observables) = run_matrix_cell(0, "unified").await;
    assert_eq!(verdict, Verdict::TipRejected, "{observables}");
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
#[tokio::test]
async fn boundary_rejection_attribution() {
    use zcash_local_net::validator::Validator as _;
    use zingo_status::confirmation_status::ConfirmationStatus;
    use zingolib::config::WalletConfig;
    use zingolib::testutils::default_test_wallet_settings;
    use zingolib_testutils::validator_rpc::{self, RawTransactionVerdict};

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

    // Reproduce the boundary rejection through the zainod path.
    let target_address = get_base_address_macro!(recipient, "unified");
    let zainod_path_error =
        from_inputs::quick_send(&mut faucet, vec![(&target_address, 20_000, None)])
            .await
            .expect_err("the boundary-adjacent orchard-output send must be rejected")
            .to_string();
    assert!(
        zainod_path_error.contains("until the next chain tip block"),
        "unexpected rejection class: {zainod_path_error}"
    );

    // Recover the exact bytes zebra judged: the wallet keeps the built
    // Transaction in the Failed record.
    let rejected_transaction_bytes = {
        let wallet = faucet.wallet();
        let wallet = wallet.read().await;
        let failed_transaction = wallet
            .wallet_transactions
            .values()
            .find(|transaction| matches!(transaction.status(), ConfirmationStatus::Failed(_)))
            .expect("the failed send must leave a Failed record holding the transaction");
        let mut bytes = vec![];
        failed_transaction.transaction().write(&mut bytes).unwrap();
        bytes
    };

    let rpc_port = local_net.validator().rpc_listen_port();

    // Predicate one, verdict parity NOW: direct submission at the
    // boundary must reproduce the rejection, exonerating zainod.
    let verdict_now =
        validator_rpc::send_raw_transaction(rpc_port, &rejected_transaction_bytes).await;
    let RawTransactionVerdict::Rejected(direct_path_error) = verdict_now else {
        panic!(
            "verdict parity falsified: zainod path rejected but direct submission              returned {verdict_now:?} — the indexer is transforming verdicts"
        );
    };
    assert!(
        direct_path_error.contains("until the next chain tip block")
            || direct_path_error.contains("orchard proof"),
        "direct rejection is a different class than the zainod-path rejection:          direct={direct_path_error} zainod={zainod_path_error}"
    );

    // Predicate two, same bytes LATER: five blocks of distance, then the
    // identical bytes. Observed and pinned: still rejected, now cleanly
    // as a wrong consensus branch id — the wallet built the transaction
    // under pre-activation consensus (H-WALLET-CONTEXT); zebra judged
    // correctly at both heights.
    local_net.validator().generate_blocks(5).await.unwrap();
    let verdict_later =
        validator_rpc::send_raw_transaction(rpc_port, &rejected_transaction_bytes).await;
    let RawTransactionVerdict::Rejected(later_error) = verdict_later else {
        panic!(
            "H-WALLET-CONTEXT falsified: the identical bytes were ACCEPTED after \
             distance from the boundary — zebra's boundary-time verdict was wrong \
             and the mechanism is inside zebra after all"
        );
    };
    assert!(
        later_error.contains("incorrect consensus branch id"),
        "rejection class changed: expected the wrong-branch-id rejection that \
         convicts the wallet-side builder, observed: {later_error}"
    );
}

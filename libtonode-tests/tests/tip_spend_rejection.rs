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

/// Runs one cell: fund the recipient with a single shielded note (ironwood
/// on this stack, where the funding send post-dates the ironwood
/// activation), age it `depth` blocks (the wallet stays synced to the tip,
/// so the anchor is always the tip), then send to the faucet's address in
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

/// Historically the in-suite reproduction of the pool_matrix orchard-row
/// failure (verdict pinned TipRejected). On the current pinned stack
/// (infrastructure rev 537f84d3, first adjudicated on the 2026-07-15 CI
/// run of PR #2466) the same cell is ACCEPTED: the boundary-adjacent
/// rejection no longer reproduces. Per ADR 0009's canary discipline this
/// pin now states the new observed truth; see
/// [`boundary_send_is_accepted`] for the attribution history.
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

/// # Round three: attribution of the boundary rejection (historical)
///
/// Round two localized the trigger (orchard-output transactions built
/// adjacent to the height-5 co-activation) but not the culprit. Round
/// three resubmitted the exact rejected bytes directly to zebra, at the
/// boundary and again five blocks later, and convicted the wallet-side
/// builder (H-WALLET-CONTEXT): the transaction was built under
/// pre-activation consensus, and zebra's "incorrect consensus branch id"
/// rejection of the identical bytes later proved zebra judged correctly
/// at both heights.
///
/// # Round four (2026-07-15, pinned stack 537f84d3)
///
/// The convicted mechanism is gone: the wallet's Ironwood-era builder
/// (ADR 0009) derives the post-activation consensus branch id, and the
/// boundary-adjacent orchard-output send is now ACCEPTED outright, first
/// observed on the PR #2466 CI run. The rejection this suite existed to
/// attribute can no longer be produced, so the forensic choreography
/// (capturing Failed-record bytes, direct RPC resubmission) has nothing
/// to grab. This test now pins the acceptance; if a boundary-time
/// rejection ever reappears, this pin fails loudly and the round-three
/// machinery in git history (2803a04fd and earlier) is the playbook.
#[tokio::test]
async fn boundary_send_is_accepted() {
    use zingolib::config::WalletConfig;
    use zingolib::testutils::default_test_wallet_settings;

    let (_local_net, mut client_builder) = scenarios::custom_clients_default().await;
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
    from_inputs::quick_send(&mut faucet, vec![(&target_address, 20_000, None)])
        .await
        .expect(
            "the boundary-adjacent orchard-output send is accepted on the pinned stack \
             (a rejection here resurrects the round-three attribution playbook)",
        );
}

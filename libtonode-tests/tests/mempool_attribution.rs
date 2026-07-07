//! Discriminators between INDEXER (zainod) and VALIDATOR (zebrad) mempool
//! behavior.
//!
//! Until this file, no test separated the indexer's mempool view from the
//! validator's; the nearest discriminator in the corpus is the block-height
//! tip-lag probe in sync.rs. The distinction matters for attribution: every
//! wallet-facing mempool observation flows through zainod, so a wallet test
//! alone cannot say whether surprising behavior originates in zebra or in
//! the indexer's polling and relaying of it.
//!
//! # The event marker that needs no extra plumbing
//!
//! zainod's send_transaction relays zebra's own acceptance verdict, so a
//! quick_send resolving Ok IS the validator-side event: at that instant the
//! transaction is in zebra's mempool. Everything zainod shows AFTER that
//! instant measures pure indexer ingestion lag.
//!
//! # Hypotheses and predicates
//!
//! - H-LAG: zainod's mempool view trails zebra's acceptance by its polling
//!   cadence (independently estimated at roughly five seconds from the
//!   assured-send poll measurements). Predicate: the delay between
//!   accepted-by-zebra and visible-in-zainod's-GetMempoolTx is measurable,
//!   bounded, and reported.
//! - H-FAITHFUL-ACCEPT: everything zebra accepts eventually appears in the
//!   indexer's mempool view (no silent drops on the accept side).
//!   Predicate: the txid appears within the bound.
//!
//! # Round-one verdict (2026-07-06, host stack)
//!
//! Observed indexer ingestion lag: ~50 MILLISECONDS. H-LAG as stated is
//! FALSIFIED — the indexer is not the source of the multi-second waits
//! the assured-send helper observes (and the poll-not-sleep commit's
//! attribution of that wait to "zainod's mempool-polling cadence" was
//! wrong; this measurement corrects the record). The surviving suspect
//! is the third leg of the pipeline, measured by the second cell below:
//!
//! - H-WALLET-MONITOR: the wallet's own mempool-monitor pipeline
//!   (stream delivery plus pepper-sync's mempool-transaction processing)
//!   is where the seconds go. Predicate: the delay between validator
//!   acceptance and the SENDER's own wallet record reaching Mempool
//!   status is much larger than the indexer's 50 ms.
//!
//! Round-two datum: with NO sync session running, the sender's record
//! never reaches Mempool status at all (first version of the wallet
//! cell timed out at 15 s) — pepper-sync's mempool monitor only exists
//! within a sync session's lifecycle. That is why the assured-send
//! helper syncs before its mempool cross-checks and still waits: the
//! wait is monitor startup and processing after sync_and_await returns.
//! The cell below therefore syncs after sending, as the helper does,
//! and measures delivery from validator acceptance.
//!
//! Round-two verdict: wallet-record lag 2.503121 s with sync_and_await
//! returning at 2.503124 s — IDENTICAL to the microsecond. The mempool
//! transaction is processed inside the sync session itself; there is no
//! post-sync monitor delay at all. The full attribution triangle on the
//! accept side: zebra accepts at t0, zainod shows the transaction at
//! t0+50ms, and the wallet record is ready the moment its next sync
//! session completes (~2.5 s, the cost of the sync itself). The
//! assured-send helper's old fixed 6-second sleep was pure waste, and
//! its poll replacement now exits immediately after the helper's own
//! sync — consistent with the observed drop in per-case time.
//!
//! # Designed cells blocked on two plumbing seams
//!
//! The rejection-side attribution cells for the boundary-adjacent
//! orchard-output phenomenon (see tip_spend_rejection.rs) need seams that
//! do not exist yet:
//!
//! - Direct-submission verdict parity (does zebra's sendrawtransaction
//!   reject the same bytes the zainod path rejects?) and same-bytes
//!   resubmission after one block (if identical bytes are later accepted,
//!   the proof was always valid and zebra's boundary-time verdict was
//!   wrong) need the validator's JSON-RPC port. CORRECTION (2026-07-07):
//!   the running Zebrad already exposes it — copy_getters generates
//!   pub fn rpc_listen_port() — so the infrastructure side is ready
//!   today; an earlier note here claimed otherwise after a grep for
//!   literal `pub fn` missed the macro-generated getter.
//! - Offline orchard-proof verification (if the wallet's proof verifies
//!   against the standard verifying key offline, the wallet is exonerated
//!   at the proof level) needs the built transaction's raw bytes, which
//!   the wallet does not retain. Seam, still missing: a
//!   build-without-broadcast test-features function in zingolib's send
//!   path. This is the only remaining blocker for the rejection-side
//!   cells.

use std::time::{Duration, Instant};

use zingo_netutils::lightwallet_protocol::GetMempoolTxRequest;
use zingo_netutils::{GrpcIndexer, Indexer};
use zingolib::get_base_address_macro;
use zingolib::testutils::lightclient::from_inputs;
use zingolib_testutils::scenarios;

/// H-LAG and H-FAITHFUL-ACCEPT: measure how long after zebra accepts a
/// transaction the indexer's mempool view shows it, and assert it shows
/// at all.
#[tokio::test]
async fn indexer_mempool_view_trails_validator_acceptance() {
    let (ref _local_net, faucet, mut recipient, _txid) =
        scenarios::faucet_funded_recipient_default(100_000).await;

    let target_address = get_base_address_macro!(faucet, "unified");
    let txids = from_inputs::quick_send(&mut recipient, vec![(&target_address, 20_000, None)])
        .await
        .unwrap();
    // quick_send resolved Ok: zebra has accepted the transaction into its
    // mempool (zainod relays zebra's verdict). Start the clock.
    let accepted_at = Instant::now();
    let txid_bytes: Vec<u8> = txids.first().as_ref().to_vec();

    let mut grpc_client = GrpcIndexer::new(recipient.indexer_uri().clone())
        .await
        .unwrap();
    let bound = Duration::from_secs(10);
    let indexer_lag = loop {
        let mut mempool_stream = grpc_client
            .get_mempool_tx(
                GetMempoolTxRequest {
                    exclude_txid_suffixes: vec![],
                    pool_types: vec![],
                },
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        let mut found = false;
        while let Some(compact_transaction) = mempool_stream.message().await.unwrap() {
            // CompactTx carries natural-order txid bytes.
            if compact_transaction.txid == txid_bytes {
                found = true;
                break;
            }
        }
        if found {
            break accepted_at.elapsed();
        }
        assert!(
            accepted_at.elapsed() < bound,
            "H-FAITHFUL-ACCEPT falsified: zebra accepted the transaction \
             {}+ seconds ago and zainod's GetMempoolTx still does not show it",
            bound.as_secs(),
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    // H-LAG observable, reported through the test name in nextest output:
    // the panic below never fires; it exists to keep the measurement in
    // the assertion so a regression (lag beyond the bound) fails loudly
    // with the number attached.
    println!("indexer mempool ingestion lag after validator acceptance: {indexer_lag:?}");
    assert!(
        indexer_lag < bound,
        "indexer mempool lag {indexer_lag:?} exceeded the {bound:?} bound"
    );
}

/// H-WALLET-MONITOR: measure how long after zebra accepts the transaction
/// the SENDER's own wallet record reaches Mempool status. Together with
/// the indexer cell this completes the attribution triangle:
/// validator-accept -> indexer-visible -> wallet-record.
#[tokio::test]
async fn wallet_mempool_record_trails_validator_acceptance() {
    use zingo_status::confirmation_status::ConfirmationStatus;

    let (ref _local_net, faucet, mut recipient, _txid) =
        scenarios::faucet_funded_recipient_default(100_000).await;

    let target_address = get_base_address_macro!(faucet, "unified");
    let txids = from_inputs::quick_send(&mut recipient, vec![(&target_address, 20_000, None)])
        .await
        .unwrap();
    let accepted_at = Instant::now();
    let txid = *txids.first();

    // The mempool monitor only runs within a sync session (round-two
    // datum above): without this sync the record never leaves
    // Transmitted status.
    recipient.sync_and_await().await.unwrap();
    let sync_returned = accepted_at.elapsed();

    let bound = Duration::from_secs(15);
    let wallet_record_lag = loop {
        let in_mempool = {
            let wallet = recipient.wallet();
            let wallet = wallet.read().await;
            wallet
                .wallet_transactions
                .get(&txid)
                .is_some_and(|transaction| {
                    matches!(transaction.status(), ConfirmationStatus::Mempool(_))
                })
        };
        if in_mempool {
            break accepted_at.elapsed();
        }
        assert!(
            accepted_at.elapsed() < bound,
            "the sender's wallet record never reached Mempool status within {bound:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    println!(
        "wallet mempool-record lag after validator acceptance: {wallet_record_lag:?} \
         (sync_and_await returned at {sync_returned:?})"
    );
    assert!(
        wallet_record_lag < bound,
        "wallet mempool-record lag {wallet_record_lag:?} exceeded the {bound:?} bound"
    );
}

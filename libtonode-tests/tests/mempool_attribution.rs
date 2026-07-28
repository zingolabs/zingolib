//! Discriminators between INDEXER (zainod) and VALIDATOR (zebrad) mempool
//! behavior.
//!
//! Until this file, no test separated the indexer's mempool view from the
//! validator's. The nearest discriminator in the corpus is the block-height
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
//! FALSIFIED: the indexer is not the source of the multi-second waits
//! the assured-send helper observes (and the poll-not-sleep commit's
//! attribution of that wait to "zainod's mempool-polling cadence" was
//! wrong. This measurement corrects the record). The surviving suspect
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
//! cell timed out at 15 s), since pepper-sync's mempool monitor only exists
//! within a sync session's lifecycle. That is why the assured-send
//! helper syncs before its mempool cross-checks and still waits: the
//! wait is monitor startup and processing after sync_and_await returns.
//! The cell below therefore syncs after sending, as the helper does,
//! and measures delivery from validator acceptance.
//!
//! Round-two verdict: wallet-record lag 2.503121 s with sync_and_await
//! returning at 2.503124 s, IDENTICAL to the microsecond. The mempool
//! transaction is processed inside the sync session itself. There is no
//! post-sync monitor delay at all. The full attribution triangle on the
//! accept side: zebra accepts at t0, zainod shows the transaction at
//! t0+50ms, and the wallet record is ready the moment its next sync
//! session completes (~2.5 s, the cost of the sync itself). The
//! assured-send helper's old fixed 6-second sleep was pure waste, and
//! its poll replacement now exits immediately after the helper's own
//! sync, consistent with the observed drop in per-case time.
//!
//! # Designed cells: one remaining
//!
//! Of the rejection-side attribution cells for the boundary-adjacent
//! orchard-output phenomenon, two are now built in tip_spend_rejection's
//! boundary_rejection_attribution: direct-submission verdict parity
//! (zebra's sendrawtransaction judging the captured bytes) and same-bytes
//! resubmission after five blocks of distance, both fed by the Failed
//! record's retained Transaction and the validator's rpc_listen_port.
//! The one cell still unbuilt is offline orchard-proof verification (a
//! proof that verifies against the standard verifying key offline
//! exonerates the wallet at the proof level). Its input is recoverable
//! from the same Failed record. An earlier note here claimed the wallet
//! does not retain the built bytes, which boundary_rejection_attribution
//! disproves.
//!
//! Update (2026-07-16): zebrad 6.0.0 removed the boundary rejection,
//! so boundary_rejection_attribution is now an acceptance sentinel and
//! the probe pair above (extracted into
//! zingolib_testutils::attribution) runs only when a send fails. The
//! offline proof-verification cell is moot unless the phenomenon
//! relapses.

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

    // Content parity, validator side: the acceptance marker predicts the
    // transaction is ALREADY in zebra's mempool; getrawmempool (direct
    // JSON-RPC, no indexer in the loop) turns that inference into an
    // observation.
    let validator_mempool = zingolib_testutils::validator_rpc::get_raw_mempool(
        _local_net.validator().rpc_listen_port(),
    )
    .await;
    let txid_display = txids.first().to_string();
    assert!(
        validator_mempool.contains(&txid_display),
        "acceptance-marker inference falsified: zebra's own mempool does not          contain {txid_display} immediately after send_transaction returned Ok          (mempool: {validator_mempool:?})"
    );

    let mut grpc_client = GrpcIndexer::new(
        recipient
            .indexer_uri()
            .expect("live-net client is connected"),
    )
    .await
    .unwrap();
    let bound = zingo_netutils::time::test::MEMPOOL_INGEST_BOUND;
    let indexer_lag = loop {
        let mut mempool_stream = grpc_client
            .get_mempool_tx(
                GetMempoolTxRequest {
                    exclude_txid_suffixes: vec![],
                    pool_types: vec![],
                },
                zingo_netutils::time::test::MEMPOOL_STREAM_BOUND,
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

    // H-LAG observable, printed below (nextest shows it on failure or
    // with --no-capture). The assertion pins the bound so a regression
    // (lag beyond it) fails loudly with the number attached.
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

    let bound = zingo_netutils::time::test::WALLET_RECORD_LAG_BOUND;
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

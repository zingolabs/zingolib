use super::{
    AttemptKind, AttemptRoute, FailureKind, FaultDomain, IndexerAttempt, IndexerHistoryHandle,
    MAX_HISTORY_ATTEMPTS,
};
use crate::destination::Host;

fn an_attempt(host: &str, outcome: Result<(), FailureKind>) -> IndexerAttempt {
    IndexerAttempt {
        unix_secs: 1_700_000_000,
        host: Host::of_host_str(host),
        route: AttemptRoute::Mixnet,
        kind: AttemptKind::Send,
        millis: 1234,
        fault_domain: outcome.is_err().then_some(FaultDomain::Destination),
        outcome,
    }
}

/// HYPOTHESIS: recorded attempts read back whole and in order, so the
/// session's history is the record its reporters wrote. Falsified if a
/// field is lost or the order differs.
#[test]
fn attempts_read_back_in_the_order_recorded() {
    let handle = IndexerHistoryHandle::default();
    let ok = an_attempt("zec.rocks", Ok(()));
    let failed = an_attempt("carover0.xyz", Err(FailureKind::Timeout));

    handle.record(&ok);
    handle.record(&failed);

    assert_eq!(handle.load(), vec![ok, failed]);
}

/// HYPOTHESIS: clones share one store, because separate reporters reach
/// the session's history through their own clone. Falsified if a clone
/// reads only what it recorded itself.
#[test]
fn clones_share_one_store() {
    let handle = IndexerHistoryHandle::default();
    let clone = handle.clone();

    clone.record(&an_attempt("zec.rocks", Ok(())));

    assert_eq!(
        handle.load().len(),
        1,
        "the original sees the clone's entry"
    );
}

/// HYPOTHESIS: the store is bounded, dropping its oldest entries once it
/// is full while the newest survive. Falsified if it grows past the cap
/// or drops the newest entry.
#[test]
fn the_store_is_bounded_and_keeps_the_newest() {
    let handle = IndexerHistoryHandle::default();
    let total = MAX_HISTORY_ATTEMPTS + 3;
    for index in 0..total {
        let mut attempt = an_attempt("zec.rocks", Ok(()));
        attempt.unix_secs = index as u64;
        handle.record(&attempt);
    }

    let loaded = handle.load();
    assert_eq!(
        loaded.len(),
        MAX_HISTORY_ATTEMPTS,
        "the store stays bounded"
    );
    assert_eq!(
        loaded.last().expect("nonempty").unix_secs,
        (total - 1) as u64,
        "the newest entry survives"
    );
    assert!(
        loaded.first().expect("nonempty").unix_secs > 0,
        "the oldest entries are the ones dropped"
    );
}

/// HYPOTHESIS: a probe that exhausts its leg budget classifies as a
/// timeout whichever wording the transport uses to say so, including
/// tonic's elapsed deadline, so the sweep's cause tally names the real
/// cause. Falsified if an elapsed deadline lands in any other family.
#[test]
fn an_elapsed_deadline_is_a_timeout() {
    assert_eq!(
        FailureKind::classify(
            "transport to zec.rocks:443 failed (transport error: deadline has elapsed)"
        ),
        FailureKind::Timeout
    );
    assert_eq!(
        FailureKind::classify("deadline has elapsed"),
        FailureKind::Timeout
    );
}

/// The classifier maps each failure family onto its token and never
/// panics on arbitrary prose.
#[test]
fn the_classifier_covers_the_failure_families() {
    assert_eq!(
        FailureKind::classify("transaction already queued for download"),
        FailureKind::Queued
    );
    assert_eq!(
        FailureKind::classify("nakednet connect timed out after 30s"),
        FailureKind::Timeout
    );
    assert_eq!(
        FailureKind::classify("status: DeadlineExceeded"),
        FailureKind::Timeout
    );
    assert_eq!(
        FailureKind::classify("SOCKS5 handshake failed"),
        FailureKind::Unreachable
    );
    assert_eq!(
        FailureKind::classify("connection refused"),
        FailureKind::Unreachable
    );
    assert_eq!(
        FailureKind::classify("transaction rejected: txid deadbeef"),
        FailureKind::Rejected
    );
    assert_eq!(
        FailureKind::classify("zebra said something novel"),
        FailureKind::Other
    );
}

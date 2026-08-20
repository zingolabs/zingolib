//! Per-indexer attempt history for the life of one session.
//!
//! Transmission arms and diagnostic probes record one entry per attempt, so
//! indexer reliability accumulates instead of dying with each error message:
//! which hosts deliver, over which route, how fast, and which category of
//! failure presented. Every entry also folds into the session's Health, which
//! the draws consult when they choose a Correspondent.
//!
//! # Privacy contract
//!
//! The history exists only in memory and ends with the process. Nothing is
//! written beside the wallet, so a record of when this wallet transmitted and
//! to which indexer never survives the session that made it. Failure prose
//! never enters an entry either: every failure is classified down to one
//! closed [`FailureKind`] token at the recording boundary, because raw server
//! text can embed transaction ids.
#![forbid(unsafe_code)]

use std::sync;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::correspondent::health;

/// The most attempts one session keeps, the oldest dropping out as newer ones
/// arrive, so a long session's history stays bounded.
pub const MAX_HISTORY_ATTEMPTS: usize = 1024;

/// The network route an attempt used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptRoute {
    /// A direct clearnet connection.
    Clearnet,
    /// Through the mixnet's SOCKS5 proxy.
    Mixnet,
}

/// What the attempt was doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptKind {
    /// A real transaction submission.
    Send,
    /// A diagnostic `GetLightdInfo` probe.
    Probe,
}

/// The closed set of failure categories an entry may carry, so raw server or
/// transport prose, which can embed transaction ids (zebrad duplicate
/// rejections quote them), is discarded at the recording boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    /// The attempt ran out of time (connect or request deadline).
    Timeout,
    /// The host could not be reached: connect, proxy, socks, or DNS failure.
    Unreachable,
    /// Delivered but unverified when patience ran out ("queued for download").
    Queued,
    /// The server answered and said no.
    Rejected,
    /// Anything the classifier does not recognize.
    Other,
}

impl FailureKind {
    /// The category's stable token.
    #[cfg_attr(not(feature = "nym"), allow(dead_code))]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            FailureKind::Timeout => "timeout",
            FailureKind::Unreachable => "unreachable",
            FailureKind::Queued => "queued",
            FailureKind::Rejected => "rejected",
            FailureKind::Other => "other",
        }
    }

    /// Classify a failure message into its category. Substring matches,
    /// like [`classify_rejection`], because the servers surface failures
    /// untyped (zingolabs/zaino#1392). The message itself is then discarded.
    ///
    /// [`classify_rejection`]: crate::lightclient::transmit
    pub(crate) fn classify(detail: &str) -> Self {
        let lowered = detail.to_ascii_lowercase();
        if lowered.contains("already queued for download") {
            return FailureKind::Queued;
        }
        if lowered.contains("timed out")
            || lowered.contains("timeout")
            || lowered.contains("deadlineexceeded")
            || lowered.contains("deadline exceeded")
            || lowered.contains("deadline has elapsed")
        {
            return FailureKind::Timeout;
        }
        if lowered.contains("connect")
            || lowered.contains("refused")
            || lowered.contains("unreachable")
            || lowered.contains("proxy")
            || lowered.contains("socks")
            || lowered.contains("dns")
        {
            return FailureKind::Unreachable;
        }
        if lowered.contains("reject") || lowered.contains("invalid") {
            return FailureKind::Rejected;
        }
        FailureKind::Other
    }
}

/// One recorded attempt against one indexer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexerAttempt {
    /// Seconds since the Unix epoch when the attempt finished.
    pub unix_secs: u64,
    /// The indexer host the attempt targeted.
    pub host: crate::correspondent::Host,
    /// The route the attempt used.
    pub route: AttemptRoute,
    /// Whether the attempt was a send or a probe.
    pub kind: AttemptKind,
    /// How long the attempt took, in milliseconds.
    pub millis: u64,
    /// `Ok(())` on success, or the sanitized failure category.
    pub outcome: Result<(), FailureKind>,
    /// Which party a failure is charged against, when the evidence says.
    pub phase: Option<health::FailurePhase>,
}

/// The current time as seconds since the Unix epoch.
pub(crate) fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// A cloneable recorder and reader of this session's indexer history, whose
/// clones all share the one store and the one Health.
#[derive(Clone, Debug, Default)]
pub struct IndexerHistoryHandle {
    /// This session's attempts, oldest first, bounded by
    /// [`MAX_HISTORY_ATTEMPTS`].
    attempts: sync::Arc<sync::Mutex<Vec<IndexerAttempt>>>,
    /// The session's Health, updated by every attempt, which the draws
    /// consult when they choose a Correspondent.
    health: sync::Arc<sync::Mutex<health::Health>>,
}

impl IndexerHistoryHandle {
    /// The session's Health, for the draws that consult it.
    #[cfg_attr(not(feature = "nym"), allow(dead_code))]
    pub(crate) fn health(&self) -> &sync::Mutex<health::Health> {
        &self.health
    }

    /// Keeps one finished attempt in this session's store and folds it into
    /// the session's Health, dropping the oldest once the store is full.
    pub(crate) fn record(&self, attempt: &IndexerAttempt) {
        self.health.lock().expect("health mutex").note(
            &attempt.host,
            attempt.outcome.is_err(),
            attempt.phase,
        );
        let mut attempts = self.attempts.lock().expect("attempts mutex");
        if attempts.len() >= MAX_HISTORY_ATTEMPTS {
            attempts.remove(0);
        }
        attempts.push(attempt.clone());
    }

    /// Every attempt this session recorded, oldest first.
    pub fn load(&self) -> Vec<IndexerAttempt> {
        self.attempts.lock().expect("attempts mutex").clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AttemptKind, AttemptRoute, FailureKind, IndexerAttempt, IndexerHistoryHandle,
        MAX_HISTORY_ATTEMPTS,
    };
    use crate::correspondent::{Host, health};

    fn an_attempt(host: &str, outcome: Result<(), FailureKind>) -> IndexerAttempt {
        IndexerAttempt {
            unix_secs: 1_700_000_000,
            host: Host::of_host_str(host),
            route: AttemptRoute::Mixnet,
            kind: AttemptKind::Send,
            millis: 1234,
            phase: outcome
                .is_err()
                .then_some(health::FailurePhase::Correspondent),
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

    /// HYPOTHESIS: recording reaches the session's Health, so charged failures
    /// accumulate there until the host crosses the threshold. Falsified if the
    /// standing is unchanged by recording, which would leave the draws blind
    /// to every attempt the reporters made.
    #[test]
    fn recording_charges_the_host_in_health() {
        let handle = IndexerHistoryHandle::default();
        let host = Host::of_host_str("carover0.xyz");
        let healthy = |handle: &IndexerHistoryHandle| {
            handle
                .health()
                .lock()
                .expect("health mutex")
                .is_healthy(&host)
        };

        for _ in 0..health::UNHEALTHY_FAILURE_THRESHOLD - 1 {
            handle.record(&an_attempt("carover0.xyz", Err(FailureKind::Unreachable)));
            assert!(healthy(&handle), "one charge short of the threshold holds");
        }
        handle.record(&an_attempt("carover0.xyz", Err(FailureKind::Unreachable)));

        assert!(!healthy(&handle), "the threshold charge lands");
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
            FailureKind::classify("clearnet connect timed out after 30s"),
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
}

//! Per-indexer attempt history for the life of one session.
//!
//! Transmission arms and diagnostic probes record one entry per attempt, so
//! indexer reliability accumulates instead of dying with each error message:
//! which hosts deliver, over which route, how fast, and which category of
//! failure presented.
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

/// The most attempts one session keeps, the oldest dropping out as newer ones
/// arrive, so a long session's history stays bounded.
pub const MAX_HISTORY_ATTEMPTS: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultDomain {
    Tunnel,
    Destination,
    Unknown,
}

pub(crate) fn fault_domain(stage: &zl_netutils::diag::NetOpStage) -> FaultDomain {
    use zl_netutils::diag::NetOpStage;
    match stage {
        NetOpStage::RouteResolution
        | NetOpStage::LocalProxyConnect
        | NetOpStage::SocksHandshake
        | NetOpStage::TunnelTransport => FaultDomain::Tunnel,
        NetOpStage::RemoteTls | NetOpStage::RemoteHttp | NetOpStage::PayloadDecode => {
            FaultDomain::Destination
        }
        _ => FaultDomain::Unknown,
    }
}

/// The network route an attempt used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttemptRoute {
    /// A direct nakednet connection.
    Nakednet,
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
    pub host: crate::destination::Host,
    /// The route the attempt used.
    pub route: AttemptRoute,
    /// Whether the attempt was a send or a probe.
    pub kind: AttemptKind,
    /// How long the attempt took, in milliseconds.
    pub millis: u64,
    /// `Ok(())` on success, or the sanitized failure category.
    pub outcome: Result<(), FailureKind>,
    /// Which component a failure is attributed to, when the evidence says.
    pub fault_domain: Option<FaultDomain>,
}

/// The current time as seconds since the Unix epoch.
pub(crate) fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// A cloneable recorder and reader of this session's indexer history, whose
/// clones all share the one store.
#[derive(Clone, Debug, Default)]
pub struct IndexerHistoryHandle {
    /// This session's attempts, oldest first, bounded by
    /// [`MAX_HISTORY_ATTEMPTS`].
    attempts: sync::Arc<sync::Mutex<Vec<IndexerAttempt>>>,
}

impl IndexerHistoryHandle {
    /// Appends one finished attempt to this session's store, dropping the
    /// oldest once the store is full.
    pub(crate) fn record(&self, attempt: &IndexerAttempt) {
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
mod tests;

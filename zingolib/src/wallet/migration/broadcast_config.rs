//! Caller-supplied broadcast routing configuration and the data the library discloses back.

/// What the migration broadcast may do when the only reachable target is the synchronization operator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SyncEndpointBroadcast {
    /// Never broadcast through the synchronization operator.
    #[default]
    Forbid,
    /// Broadcast through the synchronization operator with the caller's recorded consent to the correlation.
    AllowWithCorrelationConsent,
}

/// The caller-supplied migration broadcast candidate pool and synchronization-operator policy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MigrationBroadcastConfig {
    /// Endpoints the migration broadcast may draw from; empty means none available.
    pub candidates: Vec<http::Uri>,
    /// What to do when the only reachable target is the synchronization operator.
    pub sync_endpoint: SyncEndpointBroadcast,
}

impl MigrationBroadcastConfig {
    /// A config drawing from `candidates`, refusing to broadcast through the synchronization operator.
    pub fn new(candidates: Vec<http::Uri>) -> Self {
        MigrationBroadcastConfig {
            candidates,
            sync_endpoint: SyncEndpointBroadcast::Forbid,
        }
    }
}

/// One resolved broadcast target the plan discloses to the caller, deterministic and connectionless.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BroadcastTarget {
    /// The endpoint URI.
    pub uri: http::Uri,
    /// The accumulating-party key used for exclusion.
    pub operator: String,
    /// Whether the target answered over the mixnet, or `None` until probed.
    pub reachable_over_mixnet: Option<bool>,
}

impl BroadcastTarget {
    /// A target whose reachability has not been probed yet.
    pub fn unprobed(uri: http::Uri, operator: String) -> Self {
        BroadcastTarget {
            uri,
            operator,
            reachable_over_mixnet: None,
        }
    }
}

/// The outcome of probing a candidate over the current mixnet route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reachability {
    /// The target answered at the reported block height.
    Reachable {
        /// The block height the server reported.
        height: u64,
    },
    /// The target did not answer over the mixnet.
    Unreachable {
        /// The rendered failure.
        reason: String,
    },
}

impl Reachability {
    /// Whether the target answered over the mixnet.
    pub fn is_reachable(&self) -> bool {
        matches!(self, Reachability::Reachable { .. })
    }
}

/// Whether two hosts belong to the same accumulating operator.
pub(crate) fn same_operator(host_a: &str, host_b: &str) -> bool {
    operator_domain(host_a) == operator_domain(host_b)
}

/// The operator key of a host: its registrable parent domain (last two labels), lowercased.
pub(crate) fn operator_domain(host: &str) -> String {
    let host = host.to_ascii_lowercase();
    let labels: Vec<&str> = host.rsplit('.').collect();
    labels
        .iter()
        .take(2)
        .rev()
        .copied()
        .collect::<Vec<_>>()
        .join(".")
}

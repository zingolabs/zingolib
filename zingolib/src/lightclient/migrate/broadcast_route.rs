//! How migration parts choose their wire (ADR 0011, amendment 2026-07-23).
//!
//! Migration-part broadcasts obey the Mixnet Mode policy like every other
//! transmitting surface: while the mode is on they travel ONLY over the
//! mixnet (failing closed while it bootstraps or after the proxy dies,
//! never falling back to clearnet), and clearnet carries them only when the
//! user deliberately toggled the mode off for the session, or in a build
//! compiled without the `nym` feature.
//!
//! Each part is submitted to one endpoint drawn per submission from the caller's [`MigrationBroadcastConfig`] pool minus the synchronization operator, failing closed rather than falling back.

use crate::wallet::migration::broadcast::{BroadcastClient, BroadcastError};
use crate::wallet::migration::broadcast_config::{
    BroadcastTarget, MigrationBroadcastConfig, SyncEndpointBroadcast, operator_domain,
    same_operator,
};
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;

use super::broadcast_grpc::GrpcBroadcastClient;

use crate::lightclient::error::LightClientError;

/// The [`BroadcastClient`] the Mixnet Mode policy resolved for this session:
/// one concrete type for the callers, delegating to the wire the route chose.
pub enum RoutedBroadcastClient {
    /// Clearnet submission, the deliberate mixnet opt-out, or a build
    /// without the `nym` feature.
    Clearnet(GrpcBroadcastClient),
    /// Mixnet submission through the local SOCKS5 proxy.
    #[cfg(feature = "nym")]
    Mixnet(MixnetBroadcastClient),
}

impl BroadcastClient for RoutedBroadcastClient {
    async fn submit(
        &self,
        raw_tx: Vec<u8>,
        expiry_height: BlockHeight,
    ) -> Result<TxId, BroadcastError> {
        match self {
            RoutedBroadcastClient::Clearnet(client) => client.submit(raw_tx, expiry_height).await,
            #[cfg(feature = "nym")]
            RoutedBroadcastClient::Mixnet(client) => client.submit(raw_tx, expiry_height).await,
        }
    }
}

/// Submits parts through the local SOCKS5 proxy, one randomly drawn
/// Broadcast Indexer per submission, and can do nothing else. The ZIP 318
/// no-synchronization guarantee holds structurally here exactly as it does
/// for the clearnet client.
#[cfg(feature = "nym")]
pub struct MixnetBroadcastClient {
    socks5_addr: String,
    /// The eligible targets ([`eligible_candidates`]): nonempty, and never
    /// operated by the synchronization endpoint's operator (ADR 0022).
    candidates: Vec<http::Uri>,
}

#[cfg(feature = "nym")]
impl MixnetBroadcastClient {
    /// A client dialing through the proxy at `socks5_addr`, drawing each
    /// submission's target from `candidates`.
    pub(crate) fn new(socks5_addr: String, candidates: Vec<http::Uri>) -> Self {
        MixnetBroadcastClient {
            socks5_addr,
            candidates,
        }
    }
}

#[cfg(feature = "nym")]
impl BroadcastClient for MixnetBroadcastClient {
    async fn submit(
        &self,
        raw_tx: Vec<u8>,
        expiry_height: BlockHeight,
    ) -> Result<TxId, BroadcastError> {
        use rand::seq::SliceRandom as _;

        let indexer = self
            .candidates
            .choose(&mut rand::rngs::OsRng)
            .ok_or_else(|| BroadcastError::Transport("no broadcast candidates".to_string()))?;
        let txid_hex = zingo_netutils::send_transaction_via_socks5(
            &self.socks5_addr,
            indexer,
            &raw_tx,
            u64::from(u32::from(expiry_height)),
            super::broadcast_grpc::MIGRATION_SUBMIT_TIMEOUT,
        )
        .await
        .map_err(|error| {
            // The taxonomy's own failover reading maps onto BroadcastError's
            // contract: a failover candidate was not consumed (Transport,
            // retryable: the part falls to reconciliation), a verdict was.
            let rendered = error.to_string();
            if error.is_failover_candidate() {
                BroadcastError::Transport(rendered)
            } else {
                BroadcastError::Rejected(rendered)
            }
        })?;
        crate::utils::conversion::txid_from_hex_encoded_str(&txid_hex).map_err(|e| {
            BroadcastError::Rejected(format!("endpoint returned an invalid txid: {e}"))
        })
    }
}

/// The caller's candidate pool minus the synchronization operator, refusing with [`LightClientError::NoEligibleBroadcastIndexer`] when that leaves nothing (unless [`SyncEndpointBroadcast::AllowWithCorrelationConsent`]).
pub(crate) fn eligible_candidates(
    config: &MigrationBroadcastConfig,
    sync_indexer: Option<&http::Uri>,
) -> Result<Vec<http::Uri>, LightClientError> {
    if config.candidates.is_empty() {
        return Err(LightClientError::NoEligibleBroadcastIndexer);
    }

    // Applying operator-level exclusion to the caller's pool is the whole of
    // the mechanism: the same `same_operator` predicate the send fan-out uses,
    // so the migration draw can never diverge from it.
    let eligible: Vec<http::Uri> = match sync_indexer {
        Some(sync) => config
            .candidates
            .iter()
            .filter(|candidate| {
                !candidate
                    .host()
                    .zip(sync.host())
                    .is_some_and(|(candidate, sync)| same_operator(candidate, sync))
            })
            .cloned()
            .collect(),
        // No accumulating sync operator to exclude (an Indexerless session):
        // the no-sync-operator invariant holds vacuously over the full pool.
        None => config.candidates.clone(),
    };
    if !eligible.is_empty() {
        return Ok(eligible);
    }

    // Every candidate is under the synchronization operator.
    match config.sync_endpoint {
        SyncEndpointBroadcast::Forbid => Err(LightClientError::NoEligibleBroadcastIndexer),
        SyncEndpointBroadcast::AllowWithCorrelationConsent => Ok(config.candidates.clone()),
    }
}

/// The eligible broadcast targets a plan discloses, in draw order with reachability unprobed, empty when nothing is eligible.
pub(crate) fn broadcast_targets(
    config: &MigrationBroadcastConfig,
    sync_indexer: Option<&http::Uri>,
) -> Vec<BroadcastTarget> {
    eligible_candidates(config, sync_indexer)
        .map(|uris| {
            uris.into_iter()
                .map(|uri| {
                    let operator = uri.host().map(operator_domain).unwrap_or_default();
                    BroadcastTarget::unprobed(uri, operator)
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(text: &str) -> http::Uri {
        text.parse().expect("static uri")
    }

    fn forbid(candidates: Vec<http::Uri>) -> MigrationBroadcastConfig {
        MigrationBroadcastConfig {
            candidates,
            sync_endpoint: SyncEndpointBroadcast::Forbid,
        }
    }

    /// The sync operator is excluded case-insensitively, keeping the pool's own order.
    #[test]
    fn the_sync_operator_is_excluded_from_the_pool() {
        let config = forbid(vec![
            uri("https://zec.rocks:443"),
            uri("https://Sync.Example:443"),
            uri("https://other.example:443"),
        ]);
        let sync = uri("https://sync.example:443");
        let candidates = eligible_candidates(&config, Some(&sync)).expect("two remain");
        assert_eq!(
            candidates,
            vec![
                uri("https://zec.rocks:443"),
                uri("https://other.example:443")
            ]
        );
    }

    /// Exclusion is by operator, not exact host: a regional variant bars every host under the operator.
    #[test]
    fn the_sync_operators_regional_variant_excludes_the_operator() {
        let config = forbid(vec![
            uri("https://zec.rocks:443"),
            uri("https://na.zec.rocks:443"),
            uri("https://other.example:443"),
        ]);
        let sync = uri("https://eu.zec.rocks:443");
        let candidates = eligible_candidates(&config, Some(&sync)).expect("one remains");
        assert_eq!(candidates, vec![uri("https://other.example:443")]);
    }

    /// With no synchronization endpoint there is no operator to exclude, so the whole pool is eligible.
    #[test]
    fn no_sync_indexer_excludes_nothing() {
        let config = forbid(vec![uri("https://zec.rocks:443")]);
        assert_eq!(
            eligible_candidates(&config, None).expect("unfiltered"),
            config.candidates
        );
    }

    /// An empty candidate pool is a typed refusal, not a silent no-op.
    #[test]
    fn an_empty_pool_is_a_typed_refusal() {
        let refused = eligible_candidates(&forbid(vec![]), Some(&uri("https://sync.example:443")));
        assert!(matches!(
            refused,
            Err(LightClientError::NoEligibleBroadcastIndexer)
        ));
    }

    /// A pool of only the sync operator under `Forbid` refuses rather than falling back.
    #[test]
    fn only_the_sync_operator_under_forbid_refuses() {
        let config = forbid(vec![
            uri("https://zec.rocks:443"),
            uri("https://eu.zec.rocks:443"),
        ]);
        let sync = uri("https://na.zec.rocks:443");
        assert!(matches!(
            eligible_candidates(&config, Some(&sync)),
            Err(LightClientError::NoEligibleBroadcastIndexer)
        ));
    }

    /// The same pool under `AllowWithCorrelationConsent` returns the sync operator's candidates.
    #[test]
    fn only_the_sync_operator_under_consent_returns_the_sync_candidates() {
        let config = MigrationBroadcastConfig {
            candidates: vec![
                uri("https://zec.rocks:443"),
                uri("https://eu.zec.rocks:443"),
            ],
            sync_endpoint: SyncEndpointBroadcast::AllowWithCorrelationConsent,
        };
        let sync = uri("https://na.zec.rocks:443");
        let candidates = eligible_candidates(&config, Some(&sync)).expect("consent permits it");
        assert_eq!(candidates, config.candidates);
    }

    /// The eligible set is a pure function of the pool and sync endpoint, order stable across calls.
    #[test]
    fn the_eligible_set_is_deterministic() {
        let config = forbid(vec![
            uri("https://a.example:443"),
            uri("https://zec.rocks:443"),
            uri("https://b.example:443"),
        ]);
        let sync = uri("https://zec.rocks:443");
        let first = eligible_candidates(&config, Some(&sync)).expect("two remain");
        let second = eligible_candidates(&config, Some(&sync)).expect("two remain");
        assert_eq!(first, second);
        assert_eq!(
            first,
            vec![uri("https://a.example:443"), uri("https://b.example:443")]
        );
    }

    /// Disclosed targets label each host's operator and leave reachability unprobed.
    #[test]
    fn broadcast_targets_label_the_operator_and_leave_reachability_unprobed() {
        let config = forbid(vec![
            uri("https://eu.zec.rocks:443"),
            uri("https://other.example:443"),
        ]);
        let sync = uri("https://sync.example:443");
        let targets = broadcast_targets(&config, Some(&sync));
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].operator, "zec.rocks");
        assert_eq!(targets[1].operator, "other.example");
        assert!(targets.iter().all(|t| t.reachable_over_mixnet.is_none()));

        let empty = broadcast_targets(&forbid(vec![]), Some(&sync));
        assert!(empty.is_empty());
    }
}

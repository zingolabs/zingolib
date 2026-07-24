//! How migration parts choose their wire (ADR 0011, amendment 2026-07-23).
//!
//! Migration-part broadcasts obey the Mixnet Mode policy like every other
//! transmitting surface: while the mode is on they travel ONLY over the
//! mixnet — failing closed while it bootstraps or after the proxy dies,
//! never falling back to clearnet — and clearnet carries them only when the
//! user deliberately toggled the mode off for the session, or in a build
//! compiled without the `nym` feature.
//!
//! Over the mixnet, each part is submitted to one Broadcast Indexer drawn at
//! random per submission (Witness Rotation for migration parts), and the
//! synchronization endpoint is forbidden as a target: a configured
//! `migration_broadcast_uri` sharing the sync server's host is refused, and
//! the random draw excludes that host from the curated list. The clearnet
//! opt-out path keeps its historical behavior, including the logged
//! correlation warning when it must fall back to the sync endpoint.

use crate::wallet::migration::broadcast::{BroadcastClient, BroadcastError};
use zcash_primitives::transaction::TxId;
use zcash_protocol::consensus::BlockHeight;

use super::broadcast_grpc::GrpcBroadcastClient;

#[cfg(feature = "nym")]
use crate::lightclient::error::LightClientError;

/// The [`BroadcastClient`] the Mixnet Mode policy resolved for this session:
/// one concrete type for the callers, delegating to the wire the route chose.
pub enum RoutedBroadcastClient {
    /// Clearnet submission — the deliberate mixnet opt-out, or a build
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
/// Broadcast Indexer per submission, and can do nothing else — the ZIP 318
/// no-synchronization guarantee holds structurally here exactly as it does
/// for the clearnet client.
#[cfg(feature = "nym")]
pub struct MixnetBroadcastClient {
    socks5_addr: String,
    /// The eligible targets ([`eligible_candidates`]): nonempty, and never
    /// sharing the synchronization endpoint's host.
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
            super::broadcast_grpc::SUBMIT_TIMEOUT,
        )
        .await
        .map_err(|error| {
            // The taxonomy's own failover reading maps onto BroadcastError's
            // contract: a failover candidate was not consumed (Transport,
            // retryable — the part falls to reconciliation), a verdict was.
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

/// The mixnet targets migration parts may go to, pure over its inputs: the
/// configured `migration_broadcast_uri` alone when set, otherwise the curated
/// Broadcast Indexer pool — in both cases with the synchronization endpoint's
/// host forbidden, so no server correlates a wallet's sync stream with its
/// migration cohort.
#[cfg(feature = "nym")]
pub(crate) fn eligible_candidates(
    configured: Option<http::Uri>,
    sync_host: Option<&str>,
    pool: Vec<http::Uri>,
) -> Result<Vec<http::Uri>, LightClientError> {
    let shares_sync_host = |uri: &http::Uri| {
        uri.host()
            .zip(sync_host)
            .is_some_and(|(candidate, sync)| candidate.eq_ignore_ascii_case(sync))
    };
    if let Some(configured) = configured {
        if shares_sync_host(&configured) {
            return Err(LightClientError::MigrationBroadcastTargetIsSyncEndpoint {
                host: configured.host().unwrap_or_default().to_string(),
            });
        }
        return Ok(vec![configured]);
    }
    let candidates: Vec<http::Uri> = pool
        .into_iter()
        .filter(|uri| !shares_sync_host(uri))
        .collect();
    if candidates.is_empty() {
        return Err(LightClientError::NoEligibleBroadcastIndexer);
    }
    Ok(candidates)
}

#[cfg(all(test, feature = "nym"))]
mod tests {
    use super::*;

    fn uri(text: &str) -> http::Uri {
        text.parse().expect("static uri")
    }

    /// HYPOTHESIS: the sync endpoint's host is excluded from the random
    /// pool, case-insensitively. Falsified if a migration part could be
    /// drawn toward the server that also sees the wallet's sync stream.
    #[test]
    fn the_sync_host_is_excluded_from_the_pool() {
        let pool = vec![
            uri("https://zec.rocks:443"),
            uri("https://Sync.Example:443"),
            uri("https://other.example:443"),
        ];
        let candidates = eligible_candidates(None, Some("sync.example"), pool).expect("two remain");
        assert_eq!(
            candidates,
            vec![
                uri("https://zec.rocks:443"),
                uri("https://other.example:443")
            ]
        );
    }

    /// HYPOTHESIS: a configured broadcast target equal to the sync endpoint
    /// is refused outright, not silently accepted.
    #[test]
    fn a_configured_target_on_the_sync_host_is_refused() {
        let refused = eligible_candidates(
            Some(uri("https://sync.example:9067")),
            Some("sync.example"),
            vec![uri("https://other.example:443")],
        );
        assert!(matches!(
            refused,
            Err(LightClientError::MigrationBroadcastTargetIsSyncEndpoint { host }) if host == "sync.example"
        ));
    }

    /// A configured target on a different host is the sole candidate; the
    /// curated pool is not consulted.
    #[test]
    fn a_distinct_configured_target_is_the_sole_candidate() {
        let candidates = eligible_candidates(
            Some(uri("https://dedicated.example:443")),
            Some("sync.example"),
            vec![uri("https://other.example:443")],
        )
        .expect("the override stands alone");
        assert_eq!(candidates, vec![uri("https://dedicated.example:443")]);
    }

    /// With no sync endpoint configured there is nothing to exclude.
    #[test]
    fn no_sync_host_excludes_nothing() {
        let pool = vec![uri("https://zec.rocks:443")];
        assert_eq!(
            eligible_candidates(None, None, pool.clone()).expect("unfiltered"),
            pool
        );
    }

    /// HYPOTHESIS: emptying the pool is a typed refusal, never an empty
    /// silent no-op that would strand due parts without a diagnosis.
    #[test]
    fn an_emptied_pool_is_a_typed_refusal() {
        let refused = eligible_candidates(
            None,
            Some("only.example"),
            vec![uri("https://only.example:443")],
        );
        assert!(matches!(
            refused,
            Err(LightClientError::NoEligibleBroadcastIndexer)
        ));
    }
}

//! How migration parts choose their wire (ADR 0011, amendment 2026-07-23).
//!
//! Migration-part transmissions obey the Mixnet Mode policy like every other
//! transmitting surface: while the mode is on they travel ONLY over the
//! mixnet (failing closed while it bootstraps or after the proxy dies,
//! never falling back to clearnet), and clearnet carries them only when the
//! user deliberately toggled the mode off for the session, or in a build
//! compiled without the `nym` feature.
//!
//! Over the mixnet, each part is submitted to one Correspondent drawn at
//! random per submission (Correspondent Rotation for migration parts), and
//! the synchronization endpoint's *operator* is forbidden as a target (ADR
//! 0022): a configured `migration_transmission_uri` on the sync operator's
//! domain is refused, and the random draw excludes that operator from the
//! curated list through the same exclusion core the send escalation uses, so
//! a sync connection to a regional variant (`eu.zec.rocks`) still bars the
//! operator's Correspondent (`zec.rocks`). The clearnet opt-out path keeps
//! its historical behavior, including the logged correlation warning when it
//! must fall back to the sync endpoint.

use crate::wallet::migration::transmission::{
    PartTransmissionError, TransmissionClient, TransmissionReceipt,
};
use zcash_protocol::consensus::BlockHeight;

#[cfg(feature = "nym")]
use crate::wallet::migration::transmission::TransmissionRoute;
#[cfg(feature = "nym")]
use zcash_primitives::transaction::TxId;

use super::transmission_grpc::GrpcTransmissionClient;

#[cfg(feature = "nym")]
use crate::lightclient::error::LightClientError;

/// The [`TransmissionClient`] the Mixnet Mode policy resolved for this session:
/// one concrete type for the callers, delegating to the wire the route chose.
pub enum RoutedTransmissionClient {
    /// Clearnet submission, the deliberate mixnet opt-out, or a build
    /// without the `nym` feature.
    Clearnet(GrpcTransmissionClient),
    /// Mixnet submission through the local SOCKS5 proxy.
    #[cfg(feature = "nym")]
    Mixnet(MixnetTransmissionClient),
}

impl TransmissionClient for RoutedTransmissionClient {
    async fn submit(
        &self,
        raw_tx: Vec<u8>,
        expiry_height: BlockHeight,
    ) -> Result<TransmissionReceipt, PartTransmissionError> {
        match self {
            RoutedTransmissionClient::Clearnet(client) => {
                client.submit(raw_tx, expiry_height).await
            }
            #[cfg(feature = "nym")]
            RoutedTransmissionClient::Mixnet(client) => client.submit(raw_tx, expiry_height).await,
        }
    }
}

/// Submits parts through the local SOCKS5 proxy, one randomly drawn
/// Correspondent per submission, and can do nothing else. The ZIP 318
/// no-synchronization guarantee holds structurally here exactly as it does
/// for the clearnet client.
#[cfg(feature = "nym")]
pub struct MixnetTransmissionClient {
    /// The conduit's guard, held for the client's whole life because the
    /// client dials on every submission (ADR 0048).
    dial: zingo_netutils::conduit::ConduitDial,
    /// The eligible targets ([`eligible_candidates`]): nonempty, and never
    /// operated by the synchronization endpoint's operator (ADR 0022).
    candidates: Vec<http::Uri>,
}

#[cfg(feature = "nym")]
impl MixnetTransmissionClient {
    /// A client dialing through `dial`'s conduit, drawing each submission's
    /// target from `candidates`.
    pub(crate) fn new(
        dial: zingo_netutils::conduit::ConduitDial,
        candidates: Vec<http::Uri>,
    ) -> Self {
        MixnetTransmissionClient { dial, candidates }
    }
}

#[cfg(feature = "nym")]
impl TransmissionClient for MixnetTransmissionClient {
    async fn submit(
        &self,
        raw_tx: Vec<u8>,
        expiry_height: BlockHeight,
    ) -> Result<TransmissionReceipt, PartTransmissionError> {
        use rand::seq::SliceRandom as _;

        let indexer = self
            .candidates
            .choose(&mut rand::rngs::OsRng)
            .ok_or_else(|| {
                PartTransmissionError::Transport("no transmission candidates".to_string())
            })?;
        let txid_hex = zingo_netutils::Socks5Indexer::new(
            self.dial.socks5(),
            indexer.clone(),
            super::transmission_grpc::MIGRATION_SUBMIT_TIMEOUT,
        )
        .send_transaction(&raw_tx, u64::from(u32::from(expiry_height)))
        .await
        .map_err(|error| {
            // The taxonomy's own failover reading maps onto PartTransmissionError's
            // contract: a failover candidate was not consumed (Transport,
            // retryable: the part falls to reconciliation), a verdict was.
            let rendered = error.to_string();
            if error.is_failover_candidate() {
                PartTransmissionError::Transport(rendered)
            } else {
                PartTransmissionError::Rejected(rendered)
            }
        })?;
        let txid: TxId =
            crate::utils::conversion::txid_from_hex_encoded_str(&txid_hex).map_err(|e| {
                PartTransmissionError::Rejected(format!("endpoint returned an invalid txid: {e}"))
            })?;
        Ok(TransmissionReceipt {
            txid,
            route: TransmissionRoute::Mixnet {
                correspondent: super::transmission_grpc::host_of(indexer),
                via_socks5: self.dial.socks5().to_string(),
            },
        })
    }
}

/// The mixnet targets migration parts may go to: the configured
/// `migration_transmission_uri` alone when set, otherwise the curated
/// Correspondent census, in both cases with the synchronization endpoint's
/// operator forbidden (ADR 0022), so no server correlates a wallet's sync
/// stream with its migration cohort. An exclusion that empties the
/// candidates refuses with a typed error rather than falling back.
#[cfg(feature = "nym")]
pub(crate) fn eligible_candidates(
    configured: Option<http::Uri>,
    sync_indexer: Option<&http::Uri>,
) -> Result<Vec<http::Uri>, LightClientError> {
    candidates_from(
        configured,
        sync_indexer,
        crate::correspondent::correspondent_indexers(),
    )
}

/// Pure core of [`eligible_candidates`], over an arbitrary pool for
/// testability. The pool exclusion delegates to the sanctioned constructor's
/// core (`eligible_from`), so the migration draw can never diverge from the
/// send escalation's operator-level exclusion semantics (ADR 0022).
#[cfg(feature = "nym")]
fn candidates_from(
    configured: Option<http::Uri>,
    sync_indexer: Option<&http::Uri>,
    pool: Vec<http::Uri>,
) -> Result<Vec<http::Uri>, LightClientError> {
    use crate::correspondent::{eligible_from, same_operator};

    if let Some(configured) = configured {
        let shares_sync_operator = configured
            .host()
            .zip(sync_indexer.and_then(http::Uri::host))
            .is_some_and(|(candidate, sync)| same_operator(candidate, sync));
        if shares_sync_operator {
            return Err(
                LightClientError::MigrationTransmissionTargetIsSyncEndpoint {
                    host: configured.host().unwrap_or_default().to_string(),
                },
            );
        }
        return Ok(vec![configured]);
    }
    match sync_indexer {
        Some(sync) => eligible_from(pool, sync).map_err(LightClientError::from),
        None if pool.is_empty() => {
            Err(crate::correspondent::NoEligibleCorrespondents::EmptyPool.into())
        }
        None => Ok(pool),
    }
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
        let sync = uri("https://sync.example:443");
        let candidates = candidates_from(None, Some(&sync), pool).expect("two remain");
        assert_eq!(
            candidates,
            vec![
                uri("https://zec.rocks:443"),
                uri("https://other.example:443")
            ]
        );
    }

    /// HYPOTHESIS: exclusion is by operator, not exact host (ADR 0022). A
    /// sync connection to a regional variant must still bar the operator's
    /// listed Correspondent. Falsified if the draw weakens to exact-host matching,
    /// the regression PR #2527's review found on this path.
    #[test]
    fn the_sync_operators_regional_variant_is_excluded_from_the_pool() {
        let pool = vec![
            uri("https://zec.rocks:443"),
            uri("https://other.example:443"),
        ];
        let sync = uri("https://eu.zec.rocks:443");
        let candidates = candidates_from(None, Some(&sync), pool).expect("one remains");
        assert_eq!(candidates, vec![uri("https://other.example:443")]);
    }

    /// HYPOTHESIS: a configured transmission target equal to the sync endpoint
    /// is refused outright, not silently accepted.
    #[test]
    fn a_configured_target_on_the_sync_host_is_refused() {
        let sync = uri("https://sync.example:443");
        let refused = candidates_from(
            Some(uri("https://sync.example:9067")),
            Some(&sync),
            vec![uri("https://other.example:443")],
        );
        assert!(matches!(
            refused,
            Err(LightClientError::MigrationTransmissionTargetIsSyncEndpoint { host }) if host == "sync.example"
        ));
    }

    /// HYPOTHESIS: the configured-target refusal is also operator-level: a
    /// `migration_transmission_uri` on the sync operator's regional variant is
    /// refused, since both hosts are the same accumulating party (ADR 0022).
    #[test]
    fn a_configured_target_on_the_sync_operators_variant_is_refused() {
        let sync = uri("https://zec.rocks:443");
        let refused = candidates_from(
            Some(uri("https://eu.zec.rocks:443")),
            Some(&sync),
            vec![uri("https://other.example:443")],
        );
        assert!(matches!(
            refused,
            Err(LightClientError::MigrationTransmissionTargetIsSyncEndpoint { host }) if host == "eu.zec.rocks"
        ));
    }

    /// A configured target on a different host is the sole candidate. The
    /// curated pool is not consulted.
    #[test]
    fn a_distinct_configured_target_is_the_sole_candidate() {
        let sync = uri("https://sync.example:443");
        let candidates = candidates_from(
            Some(uri("https://dedicated.example:443")),
            Some(&sync),
            vec![uri("https://other.example:443")],
        )
        .expect("the override stands alone");
        assert_eq!(candidates, vec![uri("https://dedicated.example:443")]);
    }

    /// With no sync endpoint configured there is nothing to exclude.
    #[test]
    fn no_sync_indexer_excludes_nothing() {
        let pool = vec![uri("https://zec.rocks:443")];
        assert_eq!(
            candidates_from(None, None, pool.clone()).expect("unfiltered"),
            pool
        );
    }

    /// HYPOTHESIS: emptying the pool is a typed refusal, never an empty
    /// silent no-op that would strand due parts without a diagnosis.
    #[test]
    fn an_emptied_pool_is_a_typed_refusal() {
        let sync = uri("https://only.example:443");
        let refused = candidates_from(None, Some(&sync), vec![uri("https://only.example:443")]);
        assert!(matches!(
            refused,
            Err(LightClientError::NoEligibleCorrespondent(
                crate::correspondent::NoEligibleCorrespondents::AllBelongToSyncOperator(_)
            ))
        ));
    }

    /// The production wrapper draws from the curated list itself: syncing
    /// against a curated operator's regional variant leaves the rest of the
    /// pool, with that operator absent.
    #[test]
    fn eligible_candidates_draws_the_curated_pool_minus_the_sync_operator() {
        use crate::correspondent::CORRESPONDENT_INDEXERS;

        let sync = uri("https://eu.zec.rocks:443");
        let candidates =
            eligible_candidates(None, Some(&sync)).expect("the curated pool minus one operator");
        assert_eq!(candidates.len(), CORRESPONDENT_INDEXERS.len() - 1);
        assert!(
            candidates
                .iter()
                .all(|entry| !entry.host().unwrap().ends_with("zec.rocks")),
            "the sync operator must be absent from the migration pool"
        );
    }
}

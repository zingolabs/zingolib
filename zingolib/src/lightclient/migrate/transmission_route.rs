//! How migration parts choose their wire (ADR 0011, amendment 2026-07-23;
//! ADR 0022 as amended 2026-09-11).
//!
//! Migration-part transmissions obey the Mixnet Mode policy like every other
//! transmitting surface: while the mode is on they travel ONLY over the
//! mixnet (failing closed while it bootstraps or after the proxy dies,
//! never falling back to clearnet), and clearnet carries them only when the
//! user deliberately toggled the mode off for the session, or in a build
//! compiled without the `nym` feature.
//!
//! On either wire, each part is submitted to one Destination drawn at
//! random per submission (Destination Rotation for migration parts) from
//! the session's Destination Server set under the chain's rotation policy.
//! On mainnet the synchronization endpoint's *operator* is forbidden as a
//! target: a configured `migration_transmission_uri` on the sync operator's
//! domain is refused, and the draw excludes that operator through the same
//! set the send escalation draws from, so a sync connection to a regional
//! variant (`eu.zec.rocks`) still bars the operator's Destination
//! (`zec.rocks`).

use crate::wallet::migration::transmission::{
    PartTransmissionError, TransmissionClient, TransmissionReceipt,
};
use zcash_protocol::consensus::BlockHeight;

#[cfg(feature = "nym")]
use crate::wallet::migration::transmission::TransmissionRoute;
#[cfg(feature = "nym")]
use zcash_primitives::transaction::TxId;

use super::transmission_grpc::GrpcTransmissionClient;

use crate::destination::servers::{DestinationServerSet, RotationPolicy, Transport};
use crate::lightclient::error::LightClientError;

/// The wire the Mixnet Mode policy resolved for this session's parts.
pub enum MigrationWire {
    /// Direct submission, the deliberate mixnet opt-out, or a build
    /// without the `nym` feature.
    Clearnet,
    /// Submission through the local SOCKS5 proxy. The conduit's guard is
    /// held for the client's whole life because the client dials on every
    /// submission (ADR 0048).
    #[cfg(feature = "nym")]
    Mixnet(zingo_netutils::conduit::ConduitDial),
}

impl MigrationWire {
    /// The reachability a draw over this wire needs.
    pub(crate) fn transport(&self) -> Transport {
        match self {
            MigrationWire::Clearnet => Transport::Clearnet,
            #[cfg(feature = "nym")]
            MigrationWire::Mixnet(_) => Transport::Mixnet,
        }
    }
}

/// The [`TransmissionClient`] the Mixnet Mode policy resolved for this
/// session: one randomly drawn Destination per submission, over the wire
/// the route chose, and nothing else. The ZIP 318 no-synchronization
/// guarantee holds structurally here: the client holds no sync channel.
pub struct RoutedTransmissionClient {
    wire: MigrationWire,
    /// The eligible targets ([`candidates`]): nonempty, drawn under the
    /// chain's rotation policy.
    candidates: Vec<http::Uri>,
}

impl RoutedTransmissionClient {
    /// A client over `wire`, drawing each submission's target from
    /// `candidates`.
    pub(crate) fn new(wire: MigrationWire, candidates: Vec<http::Uri>) -> Self {
        RoutedTransmissionClient { wire, candidates }
    }

    /// Whether parts travel the mixnet.
    #[cfg(all(test, feature = "nym"))]
    pub(crate) fn is_mixnet(&self) -> bool {
        !matches!(self.wire, MigrationWire::Clearnet)
    }
}

impl TransmissionClient for RoutedTransmissionClient {
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
        match &self.wire {
            MigrationWire::Clearnet => {
                GrpcTransmissionClient::new(indexer.clone())
                    .submit(raw_tx, expiry_height)
                    .await
            }
            #[cfg(feature = "nym")]
            MigrationWire::Mixnet(dial) => {
                submit_over_socks5(dial, indexer, raw_tx, expiry_height).await
            }
        }
    }
}

#[cfg(feature = "nym")]
async fn submit_over_socks5(
    dial: &zingo_netutils::conduit::ConduitDial,
    indexer: &http::Uri,
    raw_tx: Vec<u8>,
    expiry_height: BlockHeight,
) -> Result<TransmissionReceipt, PartTransmissionError> {
    let txid_hex = zingo_netutils::Socks5Indexer::new(
        dial.socks5(),
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
            destination: super::transmission_grpc::host_of(indexer),
            via_socks5: dial.socks5().to_string(),
        },
    })
}

/// The targets migration parts may go to over `transport`: the configured
/// `migration_transmission_uri` alone when set, otherwise the session's
/// Destination Server set drawn under its policy. On mainnet the
/// synchronization endpoint's operator is forbidden either way (ADR 0022),
/// so no server correlates a wallet's sync stream with its migration
/// cohort, and a draw the exclusion empties refuses with a typed error
/// rather than falling back.
pub(crate) fn candidates(
    configured: Option<http::Uri>,
    sync_indexer: Option<&http::Uri>,
    servers: &DestinationServerSet,
    transport: Transport,
    health: &crate::destination::health::Health,
) -> Result<Vec<http::Uri>, LightClientError> {
    use crate::destination::same_operator;

    if let Some(configured) = configured {
        let shares_sync_operator = servers.policy() == RotationPolicy::ExcludeSyncOperator
            && configured
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
    servers
        .draw(transport, sync_indexer, health)
        .map_err(LightClientError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::destination::health::Health;
    use zingo_netutils::indexers::{Indexer, IndexerChain};

    fn uri(text: &str) -> http::Uri {
        text.parse().expect("static uri")
    }

    fn entry(uri: &'static str) -> Indexer {
        Indexer {
            uri,
            chain: IndexerChain::Main,
            region_key: "",
            obsolete: false,
        }
    }

    fn mainnet_set() -> DestinationServerSet {
        DestinationServerSet::from_entries(
            RotationPolicy::ExcludeSyncOperator,
            &[
                entry("https://zec.rocks:443"),
                entry("https://other.example:443"),
                entry("https://third.example:443"),
            ],
        )
    }

    /// HYPOTHESIS: exclusion is by operator, not exact host (ADR 0022). A
    /// sync connection to a regional variant must still bar the operator's
    /// listed Destination, on both transports. Falsified if the draw
    /// weakens to exact-host matching, the regression PR #2527's review
    /// found on this path.
    #[test]
    fn the_sync_operators_regional_variant_is_excluded_from_the_draw() {
        let sync = uri("https://eu.zec.rocks:443");
        for transport in [Transport::Mixnet, Transport::Clearnet] {
            let drawn = candidates(
                None,
                Some(&sync),
                &mainnet_set(),
                transport,
                &Health::default(),
            )
            .expect("two remain");
            assert_eq!(
                drawn,
                vec![
                    uri("https://other.example:443"),
                    uri("https://third.example:443")
                ]
            );
        }
    }

    /// HYPOTHESIS: a configured transmission target equal to the sync endpoint
    /// is refused outright on mainnet, not silently accepted.
    #[test]
    fn a_configured_target_on_the_sync_host_is_refused() {
        let sync = uri("https://sync.example:443");
        let refused = candidates(
            Some(uri("https://sync.example:9067")),
            Some(&sync),
            &mainnet_set(),
            Transport::Clearnet,
            &Health::default(),
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
        let refused = candidates(
            Some(uri("https://eu.zec.rocks:443")),
            Some(&sync),
            &mainnet_set(),
            Transport::Mixnet,
            &Health::default(),
        );
        assert!(matches!(
            refused,
            Err(LightClientError::MigrationTransmissionTargetIsSyncEndpoint { host }) if host == "eu.zec.rocks"
        ));
    }

    /// A test chain never refuses a configured target on the sync
    /// operator: its policy allows the operator, so the refusal would bar
    /// the only server there is.
    #[test]
    fn a_test_chain_accepts_a_configured_target_on_the_sync_operator() {
        let sync = uri("https://testnet.zec.rocks:443");
        let set = DestinationServerSet::from_entries(RotationPolicy::IncludeSyncIndexer, &[]);
        let drawn = candidates(
            Some(uri("https://testnet.zec.rocks:443")),
            Some(&sync),
            &set,
            Transport::Mixnet,
            &Health::default(),
        )
        .expect("the test chain keeps its one server");
        assert_eq!(drawn, vec![sync]);
    }

    /// A configured target on a different host is the sole candidate. The
    /// set is not consulted.
    #[test]
    fn a_distinct_configured_target_is_the_sole_candidate() {
        let sync = uri("https://sync.example:443");
        let drawn = candidates(
            Some(uri("https://dedicated.example:443")),
            Some(&sync),
            &mainnet_set(),
            Transport::Clearnet,
            &Health::default(),
        )
        .expect("the override stands alone");
        assert_eq!(drawn, vec![uri("https://dedicated.example:443")]);
    }

    /// With no sync endpoint configured there is nothing to exclude.
    #[test]
    fn no_sync_indexer_excludes_nothing() {
        let drawn = candidates(
            None,
            None,
            &mainnet_set(),
            Transport::Mixnet,
            &Health::default(),
        )
        .expect("unfiltered");
        assert_eq!(drawn.len(), 3);
    }

    /// HYPOTHESIS: emptying the draw is a typed refusal, never an empty
    /// silent no-op that would strand due parts without a diagnosis.
    #[test]
    fn an_emptied_draw_is_a_typed_refusal() {
        let sync = uri("https://only.example:443");
        let set = DestinationServerSet::from_entries(
            RotationPolicy::ExcludeSyncOperator,
            &[entry("https://only.example:443")],
        );
        let refused = candidates(
            None,
            Some(&sync),
            &set,
            Transport::Mixnet,
            &Health::default(),
        );
        assert!(matches!(
            refused,
            Err(LightClientError::NoEligibleDestination(
                crate::destination::servers::NoEligibleDestinations::AllBelongToSyncOperator(_)
            ))
        ));
    }

    /// The production set draws the registry itself: syncing against a
    /// registry operator's regional variant leaves the rest of the set,
    /// with that operator absent.
    #[test]
    fn the_mainnet_set_draws_the_registry_minus_the_sync_operator() {
        let set = DestinationServerSet::for_chain(&crate::config::ChainType::Mainnet);
        let sync = uri("https://eu.zec.rocks:443");
        let drawn = candidates(
            None,
            Some(&sync),
            &set,
            Transport::Mixnet,
            &Health::default(),
        )
        .expect("the registry minus one operator");
        assert_eq!(drawn.len(), set.reachable(Transport::Mixnet).len() - 1);
        assert!(
            drawn
                .iter()
                .all(|entry| !entry.host().unwrap().ends_with("zec.rocks")),
            "the sync operator must be absent from the migration draw"
        );
    }
}

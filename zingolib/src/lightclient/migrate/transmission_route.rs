//! How migration parts choose their wire (ADR 0011, amendment 2026-07-23).
//!
//! Migration-part transmissions obey the Mixnet Mode policy like every other
//! transmitting surface: while the mode is on they travel ONLY over the
//! mixnet (failing closed while it bootstraps or after the proxy dies,
//! never falling back to clearnet), and clearnet carries them only when the
//! user deliberately toggled the mode off for the session, or in a build
//! compiled without the `nym` feature.

use crate::wallet::migration::transmission::{
    PartTransmissionError, TransmissionClient, TransmissionReceipt,
};
use zcash_protocol::consensus::BlockHeight;

#[cfg(feature = "nym")]
use crate::wallet::migration::transmission::TransmissionRoute;
#[cfg(feature = "nym")]
use zcash_primitives::transaction::TxId;

use super::transmission_grpc::GrpcTransmissionClient;

use crate::destination::servers::{
    DestinationServerSet, Location, NoEligibleDestinations, Transport, Trust,
};
use crate::lightclient::error::LightClientError;

/// The wire migration parts travel.
pub enum MigrationWire {
    /// Direct submission.
    Clearnet,
    /// Submission through the local SOCKS5 proxy.
    #[cfg(feature = "nym")]
    Mixnet(zingo_netutils::conduit::ConduitDial),
}

impl MigrationWire {
    pub(crate) fn transport(&self) -> Transport {
        match self {
            MigrationWire::Clearnet => Transport::Clearnet,
            #[cfg(feature = "nym")]
            MigrationWire::Mixnet(_) => Transport::Mixnet,
        }
    }
}

/// The [`TransmissionClient`] the Mixnet Mode policy resolved for this session.
pub struct RoutedTransmissionClient {
    wire: MigrationWire,
    candidates: Vec<http::Uri>,
}

impl RoutedTransmissionClient {
    pub(crate) fn new(wire: MigrationWire, candidates: Vec<http::Uri>) -> Self {
        RoutedTransmissionClient { wire, candidates }
    }

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

/// The targets migration parts may go to over `transport`.
pub(crate) fn candidates(
    configured: Option<http::Uri>,
    sync_indexer: Option<&http::Uri>,
    servers: &DestinationServerSet,
    transport: Transport,
    health: &crate::destination::health::Health,
) -> Result<Vec<http::Uri>, LightClientError> {
    use crate::destination::same_operator;

    if let Some(configured) = configured {
        let shares_untrusted_sync_operator = transport == Transport::Mixnet
            && sync_indexer.is_some_and(|sync| {
                servers.trust_of(sync) == Trust::Untrusted
                    && configured
                        .host()
                        .zip(sync.host())
                        .is_some_and(|(candidate, sync)| same_operator(candidate, sync))
            });
        if shares_untrusted_sync_operator {
            return Err(
                LightClientError::MigrationTransmissionTargetIsSyncEndpoint {
                    host: configured.host().unwrap_or_default().to_string(),
                },
            );
        }
        if transport == Transport::Mixnet && Location::of_uri(&configured) == Location::Local {
            return Err(NoEligibleDestinations::Empty(Transport::Mixnet).into());
        }
        return Ok(vec![configured]);
    }
    servers
        .draw(transport, sync_indexer, health)
        .map(|draw| draw.destinations().to_vec())
        .map_err(LightClientError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::destination::health::Health;
    use crate::destination::servers::IndexerConfig;

    fn uri(text: &str) -> http::Uri {
        text.parse().expect("static uri")
    }

    fn mainnet_set() -> DestinationServerSet {
        DestinationServerSet::from_uris(
            Trust::Untrusted,
            [
                uri("https://zec.rocks:443"),
                uri("https://other.example:443"),
                uri("https://third.example:443"),
            ],
        )
    }

    #[test]
    fn the_sync_operators_regional_variant_is_excluded_over_the_mixnet() {
        let sync = uri("https://eu.zec.rocks:443");
        let drawn = candidates(
            None,
            Some(&sync),
            &mainnet_set(),
            Transport::Mixnet,
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

    #[test]
    fn clearnet_parts_go_to_the_sync_indexer() {
        let sync = uri("https://eu.zec.rocks:443");
        let drawn = candidates(
            None,
            Some(&sync),
            &mainnet_set(),
            Transport::Clearnet,
            &Health::default(),
        )
        .expect("the sync indexer broadcasts");
        assert_eq!(drawn, vec![sync]);
    }

    #[test]
    fn a_configured_target_on_the_untrusted_sync_host_is_refused_over_the_mixnet() {
        let sync = uri("https://sync.example:443");
        let refused = candidates(
            Some(uri("https://sync.example:9067")),
            Some(&sync),
            &mainnet_set(),
            Transport::Mixnet,
            &Health::default(),
        );
        assert!(matches!(
            refused,
            Err(LightClientError::MigrationTransmissionTargetIsSyncEndpoint { host }) if host == "sync.example"
        ));
    }

    #[test]
    fn a_configured_target_on_the_untrusted_sync_operators_variant_is_refused() {
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

    #[test]
    fn a_configured_target_on_the_sync_host_is_used_when_the_rule_allows() {
        let sync = uri("https://node.mine.example:443");
        let target = uri("https://node.mine.example:9067");
        let clearnet = candidates(
            Some(target.clone()),
            Some(&sync),
            &mainnet_set(),
            Transport::Clearnet,
            &Health::default(),
        )
        .expect("clearnet keeps the target");
        assert_eq!(clearnet, vec![target.clone()]);
        let trusted =
            mainnet_set().with_indexer(IndexerConfig::new(sync.clone()).trust(Trust::Trusted));
        let mixnet = candidates(
            Some(target.clone()),
            Some(&sync),
            &trusted,
            Transport::Mixnet,
            &Health::default(),
        )
        .expect("a trusted sync host allows its own target");
        assert_eq!(mixnet, vec![target]);
    }

    #[test]
    fn a_local_configured_target_is_refused_over_the_mixnet_only() {
        let lan = uri("http://192.168.1.10:9067");
        assert!(matches!(
            candidates(
                Some(lan.clone()),
                None,
                &mainnet_set(),
                Transport::Mixnet,
                &Health::default(),
            ),
            Err(LightClientError::NoEligibleDestination(
                NoEligibleDestinations::Empty(Transport::Mixnet)
            ))
        ));
        assert_eq!(
            candidates(
                Some(lan.clone()),
                None,
                &mainnet_set(),
                Transport::Clearnet,
                &Health::default(),
            )
            .expect("clearnet reaches the local target"),
            vec![lan]
        );
    }

    #[test]
    fn a_distinct_configured_target_is_the_sole_candidate() {
        let sync = uri("https://sync.example:443");
        let drawn = candidates(
            Some(uri("https://dedicated.example:443")),
            Some(&sync),
            &mainnet_set(),
            Transport::Mixnet,
            &Health::default(),
        )
        .expect("the override stands alone");
        assert_eq!(drawn, vec![uri("https://dedicated.example:443")]);
    }

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

    #[test]
    fn an_emptied_draw_is_a_typed_refusal() {
        let sync = uri("https://only.example:443");
        let set =
            DestinationServerSet::from_uris(Trust::Untrusted, [uri("https://only.example:443")]);
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

    #[test]
    fn the_mainnet_set_draws_the_registry_minus_the_sync_operator() {
        let set =
            DestinationServerSet::for_chain(&crate::config::ChainType::Mainnet, None, Vec::new());
        let sync = uri("https://eu.zec.rocks:443");
        let drawn = candidates(
            None,
            Some(&sync),
            &set,
            Transport::Mixnet,
            &Health::default(),
        )
        .expect("the registry minus one operator");
        assert_eq!(
            drawn.len(),
            set.registry_reachable(Transport::Mixnet).len() - 1
        );
        assert!(
            drawn
                .iter()
                .all(|entry| !entry.host().unwrap().ends_with("zec.rocks")),
            "the sync operator must be absent from the migration draw"
        );
    }
}

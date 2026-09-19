//! How migration transfers choose their wire (ADR 0011, amendment 2026-07-23).
//!
//! Migration-transfer transmissions obey the Mixnet Mode policy like every other
//! transmitting surface: while the mode is on they travel ONLY over the
//! mixnet (failing closed while it bootstraps or after the proxy dies,
//! never falling back to clearnet), and clearnet carries them only when the
//! user deliberately toggled the mode off for the session, or in a build
//! compiled without the `nym` feature.

use crate::wallet::migration::broadcast::{
    BroadcastClient, BroadcastReceipt, TransferBroadcastError,
};
use zcash_protocol::consensus::BlockHeight;

#[cfg(feature = "nym")]
use crate::wallet::migration::broadcast::BroadcastRoute;

use super::broadcast_grpc::GrpcBroadcastClient;

use crate::destination::servers::{
    DestinationServerSet, Location, NoEligibleDestinations, Transport, Trust,
};
use crate::lightclient::error::LightClientError;

/// The wire migration transfers travel.
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

/// The [`BroadcastClient`] the Mixnet Mode policy resolved for this session.
pub struct RoutedBroadcastClient {
    wire: MigrationWire,
    candidates: Vec<http::Uri>,
}

impl RoutedBroadcastClient {
    pub(crate) fn new(wire: MigrationWire, candidates: Vec<http::Uri>) -> Self {
        RoutedBroadcastClient { wire, candidates }
    }
}

impl BroadcastClient for RoutedBroadcastClient {
    async fn submit(
        &self,
        raw_tx: Vec<u8>,
        expiry_height: BlockHeight,
    ) -> Result<BroadcastReceipt, TransferBroadcastError> {
        use rand::seq::SliceRandom as _;

        let mut candidates = self.candidates.clone();
        candidates.shuffle(&mut rand::rngs::OsRng);
        let mut last = TransferBroadcastError::NoCandidates;
        for indexer in &candidates {
            let submitted = match &self.wire {
                MigrationWire::Clearnet => {
                    GrpcBroadcastClient::new(indexer.clone())
                        .submit(raw_tx.clone(), expiry_height)
                        .await
                }
                #[cfg(feature = "nym")]
                MigrationWire::Mixnet(dial) => {
                    submit_over_socks5(dial, indexer, raw_tx.clone(), expiry_height).await
                }
            };
            match submitted {
                Err(error @ TransferBroadcastError::Transport { .. }) => last = error,
                verdict => return verdict,
            }
        }
        Err(last)
    }
}

#[cfg(feature = "nym")]
async fn submit_over_socks5(
    dial: &zingo_netutils::conduit::ConduitDial,
    indexer: &http::Uri,
    raw_tx: Vec<u8>,
    expiry_height: BlockHeight,
) -> Result<BroadcastReceipt, TransferBroadcastError> {
    let route = BroadcastRoute::Mixnet {
        destination: super::broadcast_grpc::host_of(indexer),
        via_socks5: dial.socks5().to_string(),
    };
    let txid_hex = zingo_netutils::Socks5Indexer::new(
        dial.socks5(),
        indexer.clone(),
        super::broadcast_grpc::MIGRATION_SUBMIT_TIMEOUT,
    )
    .send_transaction(&raw_tx, u64::from(u32::from(expiry_height)))
    .await
    .map_err(|error| {
        // The taxonomy's own failover reading maps onto TransferBroadcastError's
        // contract: a failover candidate was not consumed (Transport,
        // retryable: the transfer falls to reconciliation), a verdict was.
        let message = error.to_string();
        if error.is_failover_candidate() {
            TransferBroadcastError::Transport {
                route: route.clone(),
                message,
            }
        } else {
            super::broadcast_grpc::rejection(message, route.clone())
        }
    })?;
    super::broadcast_grpc::receipt_of(&txid_hex, route)
}

/// The targets migration transfers may go to over `transport`.
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

    mod failover {
        use super::*;
        use crate::testutils::mock_indexer::{MockNet, Rules, faucet_funding_transaction};
        use crate::testutils::synthetic_wallet::SyntheticWalletBuilder;
        use crate::wallet::keys::unified::ReceiverSelection;
        use crate::wallet::migration::broadcast::BroadcastRoute;

        const EXPIRY: BlockHeight = BlockHeight::from_u32(1);

        async fn unreachable_uri() -> http::Uri {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("an ephemeral localhost port binds");
            let port = listener
                .local_addr()
                .expect("bound socket has an address")
                .port();
            drop(listener);
            uri(&format!("http://127.0.0.1:{port}"))
        }

        async fn accepting_net() -> MockNet {
            let net = MockNet::launch().await;
            net.chain.write().await.rules = Rules::LAX;
            net
        }

        async fn transfer_bytes() -> Vec<u8> {
            let mut external_wallet =
                SyntheticWalletBuilder::new(zingo_test_vectors::seeds::ABANDON_ART_SEED).build();
            let (_, unified_address) = external_wallet
                .generate_unified_address(ReceiverSelection::orchard_only(), zip32::AccountId::ZERO)
                .unwrap();
            let address = unified_address.encode(&external_wallet.chain_type());
            faucet_funding_transaction(vec![(address.as_str(), 20_000, None)]).await
        }

        fn clearnet(candidates: Vec<http::Uri>) -> RoutedBroadcastClient {
            RoutedBroadcastClient::new(MigrationWire::Clearnet, candidates)
        }

        #[tokio::test]
        async fn submit_fails_over_past_unreachable_candidates_to_the_one_that_accepts() {
            let accepting = accepting_net().await;
            let candidates = vec![
                unreachable_uri().await,
                unreachable_uri().await,
                accepting.indexer_uri(),
            ];
            let transfer = transfer_bytes().await;

            let receipt = clearnet(candidates)
                .submit(transfer, EXPIRY)
                .await
                .expect("the reachable candidate takes the transfer");

            assert_eq!(
                receipt.route,
                BroadcastRoute::Clearnet {
                    endpoint: "127.0.0.1".to_string()
                }
            );
            assert_eq!(accepting.chain.read().await.mempool_len(), 1);
        }

        #[tokio::test]
        async fn submit_with_every_candidate_unreachable_reports_the_transport_failure() {
            let candidates = vec![unreachable_uri().await, unreachable_uri().await];

            let refused = clearnet(candidates)
                .submit(vec![0xAB; 64], EXPIRY)
                .await
                .expect_err("no candidate is reachable");

            assert!(
                matches!(refused, TransferBroadcastError::Transport { .. }),
                "an exhausted candidate list surfaces the last transport error, got {refused:?}"
            );
        }

        #[tokio::test]
        async fn submit_with_no_candidates_reports_no_candidates() {
            let refused = clearnet(Vec::new())
                .submit(vec![0xAB; 64], EXPIRY)
                .await
                .expect_err("nothing to submit to");

            assert!(
                matches!(refused, TransferBroadcastError::NoCandidates),
                "{refused:?}"
            );
        }

        #[tokio::test]
        async fn a_rejection_is_returned_at_once_without_trying_another_candidate() {
            let first = MockNet::launch().await;
            let second = MockNet::launch().await;
            for net in [&first, &second] {
                net.chain.write().await.reject_all_sends = true;
            }
            let candidates = vec![
                unreachable_uri().await,
                first.indexer_uri(),
                second.indexer_uri(),
            ];

            let refused = clearnet(candidates)
                .submit(vec![0xAB; 64], EXPIRY)
                .await
                .expect_err("every reachable candidate rejects");

            assert!(
                matches!(refused, TransferBroadcastError::Rejected { .. }),
                "a rejection is a verdict, not a transport failure: {refused:?}"
            );
            let contacted =
                first.chain.read().await.rejected_sends + second.chain.read().await.rejected_sends;
            assert_eq!(
                contacted, 1,
                "the first rejection ends the attempt; the other candidate is never contacted"
            );
        }

        #[tokio::test]
        async fn a_rejection_after_a_transport_failure_is_the_rejection() {
            let rejecting = MockNet::launch().await;
            rejecting.chain.write().await.reject_all_sends = true;
            let candidates = vec![unreachable_uri().await, rejecting.indexer_uri()];

            let refused = clearnet(candidates)
                .submit(vec![0xAB; 64], EXPIRY)
                .await
                .expect_err("the reachable candidate rejects");

            assert!(
                matches!(refused, TransferBroadcastError::Rejected { .. }),
                "the rejection replaces the earlier transport failure: {refused:?}"
            );
            assert_eq!(rejecting.chain.read().await.rejected_sends, 1);
        }
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

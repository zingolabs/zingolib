//! The Destination Server set: the Destinations one session may draw a
//! Transmission's target from.
//!
//! The set is derived, never curated by hand: it is the indexer registry
//! (`zingo_netutils::indexers`) partitioned to the session's chain, one
//! entry per operator. It is built once when the session opens and lives
//! only in memory, so it follows every registry update and writes nothing
//! beside the wallet. Every surface that hands a raw transaction to a
//! drawn indexer, on every transport, draws through
//! [`DestinationServerSet::draw`] (ADR 0022 as amended 2026-09-11, ADR
//! 0050).
//!
//! # Trusted and untrusted
//!
//! The set has two halves. The untrusted half is the registry: public
//! operators the wallet spreads its sends across so no one of them holds
//! both the sync view and the broadcast. The trusted half is servers the
//! user vouches for, which may hold both because they are the user's own
//! party; a draw with a trusted member reachable over the wire goes there
//! alone, with no exclusion and no rotation, and a dead trusted member is
//! a typed failure rather than a fall-through to public operators. Trust
//! is asserted by configuration, never inferred from registry membership.
//! The trusted half is wired in code and not yet fed by any configuration
//! (ruling 2026-09-12): every production session builds it empty, so the
//! live draw is the untrusted branch.
//!
//! # Reachability is a transport fact
//!
//! A 2026-07-21 paired clearnet/mixnet probe found a clean split: every
//! port-443 Destination answered over the mixnet, while every port-9067
//! entry completed the SOCKS5 tunnel and then failed the TLS handshake. The
//! exit gateways relay the standard port and mishandle the lightwalletd
//! one. Those hosts answer on clearnet, so the restriction belongs to the
//! draw's transport argument, not to the set: a mixnet draw keeps port 443
//! only and a clearnet draw keeps every member.
//!
//! # Operator diversity
//!
//! The party Destination Rotation defends against is the operator, not the
//! DNS name, so the set holds one endpoint per operator: a uniform pick
//! over an operator-diverse set spreads sends across accumulating parties,
//! where several regional endpoints of one operator would overweight it.
//! Operator identity is inferred from the registrable parent domain and is
//! ultimately self-asserted; a sybil operator running several entries would
//! weaken rotation.

use http::Uri;
use zingo_netutils::indexers::{Indexer, IndexerChain};

use super::Operator;
use super::health::Health;
use crate::config::ChainType;

/// The one port the mixnet exit policy carries (ADR 0029).
const MIXNET_PORT: u16 = 443;

/// The wire a draw must be reachable over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// A direct connection: every member is reachable.
    Clearnet,
    /// Through the mixnet's SOCKS5 tunnel: port 443 only.
    Mixnet,
}

/// What the chain lets the untrusted draw do about the sync indexer. The
/// adversary model behind Destination Rotation (ADR 0011) is a mainnet
/// concern; the test chains carry no value and have no operator-diverse
/// registry to rotate over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotationPolicy {
    /// Mainnet: the sync indexer's operator never receives a Transmission,
    /// and a draw the exclusion empties refuses (ADR 0022).
    ExcludeSyncOperator,
    /// Testnet: the registry's members and the sync indexer together, one
    /// entry per operator.
    IncludeSyncIndexer,
    /// Regtest: the registry lists nothing, so the configured sync indexer
    /// is implicitly trusted and is the sole Destination.
    SyncIndexerOnly,
}

impl RotationPolicy {
    /// The policy `chain` rules.
    pub fn for_chain(chain: &ChainType) -> Self {
        match chain {
            ChainType::Mainnet => RotationPolicy::ExcludeSyncOperator,
            ChainType::Testnet => RotationPolicy::IncludeSyncIndexer,
            ChainType::Regtest(_) => RotationPolicy::SyncIndexerOnly,
        }
    }
}

/// The registry partition `chain` reads; `None` for regtest, which the
/// registry does not carry.
fn registry_chain(chain: &ChainType) -> Option<IndexerChain> {
    match chain {
        ChainType::Mainnet => Some(IndexerChain::Main),
        ChainType::Testnet => Some(IndexerChain::Test),
        ChainType::Regtest(_) => None,
    }
}

/// One member: where it is addressed and who answers for it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct DestinationServer {
    uri: Uri,
    operator: Operator,
}

impl DestinationServer {
    fn reachable_over(&self, transport: Transport) -> bool {
        match transport {
            Transport::Clearnet => true,
            Transport::Mixnet => {
                self.uri.scheme_str() == Some("https")
                    && self.uri.port_u16().unwrap_or(MIXNET_PORT) == MIXNET_PORT
            }
        }
    }
}

/// Nothing safe to draw for a transmission, so the surface refuses rather
/// than transmit somewhere the policy forbids.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NoEligibleDestinations {
    /// Every reachable member belongs to the sync indexer's operator.
    #[error(
        "no eligible Destination: every reachable entry belongs to the \
         sync indexer's operator ({0}), and a Destination is never \
         allowed to be the sync indexer"
    )]
    AllBelongToSyncOperator(Operator),
    /// The chain has no member reachable over the transport, before any
    /// exclusion applied.
    #[error("no eligible Destination: the set holds nothing reachable over {0:?}")]
    Empty(Transport),
    /// The chain's only Destination is the sync indexer, and none is
    /// configured.
    #[error(
        "no eligible Destination: this chain transmits to the sync indexer, and none is configured"
    )]
    NoSyncIndexer,
}

/// The Destinations a session may transmit to: the trusted servers the
/// user vouched for, and the registry's entries for its chain, one per
/// operator, drawn under the chain's [`RotationPolicy`] when no trusted
/// server is reachable.
#[derive(Clone, Debug)]
pub struct DestinationServerSet {
    policy: RotationPolicy,
    trusted: Vec<DestinationServer>,
    registry: Vec<DestinationServer>,
}

fn members_of(uris: impl IntoIterator<Item = Uri>) -> Vec<DestinationServer> {
    uris.into_iter()
        .filter_map(|uri| {
            let operator = Operator::of_uri(&uri)?;
            Some(DestinationServer { uri, operator })
        })
        .collect()
}

/// `members` reachable over `transport`, the first entry of each operator
/// only, in the order given.
fn one_per_operator(members: &[DestinationServer], transport: Transport) -> Vec<Uri> {
    let mut seen: Vec<&Operator> = Vec::new();
    members
        .iter()
        .filter(|member| member.reachable_over(transport))
        .filter(|member| {
            if seen.contains(&&member.operator) {
                false
            } else {
                seen.push(&member.operator);
                true
            }
        })
        .map(|member| member.uri.clone())
        .collect()
}

impl DestinationServerSet {
    /// The set for a session on `chain`, read from the registry, with no
    /// trusted server: the shape every production session builds today.
    pub fn for_chain(chain: &ChainType) -> Self {
        let entries = registry_chain(chain)
            .into_iter()
            .flat_map(zingo_netutils::indexers::active);
        Self::from_entries(RotationPolicy::for_chain(chain), entries)
    }

    /// A set over `entries` under `policy`, the seam a test injects a
    /// registry through. Entries are kept in the order given; the first
    /// entry of an operator reachable over a transport is the one that
    /// operator's draw addresses.
    pub(crate) fn from_entries<'a>(
        policy: RotationPolicy,
        entries: impl IntoIterator<Item = &'a Indexer>,
    ) -> Self {
        Self::from_uris(
            policy,
            entries
                .into_iter()
                .filter_map(|entry| entry.uri.parse::<Uri>().ok()),
        )
    }

    /// A set whose registry is `uris` under `policy`, for a test whose
    /// Destinations are mock indexers on ports chosen at run time.
    pub fn from_uris(policy: RotationPolicy, uris: impl IntoIterator<Item = Uri>) -> Self {
        DestinationServerSet {
            policy,
            trusted: Vec::new(),
            registry: members_of(uris),
        }
    }

    /// The set with `uris` as its trusted servers. Nothing in production
    /// calls this yet (ruling 2026-09-12): the configuration that would
    /// let a user vouch for a server is not wired, so it serves the tests
    /// that pin the trusted branch until it is.
    pub fn with_trusted(mut self, uris: impl IntoIterator<Item = Uri>) -> Self {
        self.trusted = members_of(uris);
        self
    }

    /// The policy the untrusted draw runs under.
    pub fn policy(&self) -> RotationPolicy {
        self.policy
    }

    /// The trusted servers, in the order given.
    pub fn trusted(&self) -> Vec<Uri> {
        self.trusted
            .iter()
            .map(|member| member.uri.clone())
            .collect()
    }

    /// Every registry member reachable over `transport`, one per
    /// operator, before any exclusion or Health applies: the view a
    /// diagnostic that carries no wallet data (the `network probe`
    /// pairing) measures. A transmission never draws from this directly.
    pub(crate) fn reachable(&self, transport: Transport) -> Vec<Uri> {
        one_per_operator(&self.registry, transport)
    }

    /// The Destinations one transmission may contact over `transport`:
    /// the trusted servers reachable over it when there are any, else the
    /// registry with `sync_indexer` handled as the policy rules and
    /// `health`'s floor applied, in registry order for the caller's own
    /// shuffle.
    ///
    /// A trusted server that is reachable and dead fails the transmission
    /// typed; it never falls through to the registry, since that would
    /// hand the send to public operators exactly when the user chose
    /// otherwise. A trusted server the wire cannot reach (a LAN node over
    /// the mixnet) does fall through: the tunnel hides the client and the
    /// node never sees the send.
    ///
    /// The sync indexer is read at draw time rather than at construction,
    /// so a session that rebinds its sync indexer (the Server-Selection
    /// Sweep does) never draws against a stale exclusion.
    pub fn draw(
        &self,
        transport: Transport,
        sync_indexer: Option<&Uri>,
        health: &Health,
    ) -> Result<Vec<Uri>, NoEligibleDestinations> {
        let trusted = one_per_operator(&self.trusted, transport);
        if !trusted.is_empty() {
            return Ok(trusted);
        }
        let pool = match self.policy {
            RotationPolicy::SyncIndexerOnly => {
                return sync_indexer
                    .cloned()
                    .map(|uri| vec![uri])
                    .ok_or(NoEligibleDestinations::NoSyncIndexer);
            }
            RotationPolicy::IncludeSyncIndexer => {
                let with_sync = members_of(sync_indexer.cloned())
                    .into_iter()
                    .chain(self.registry.iter().cloned())
                    .collect::<Vec<_>>();
                let reachable = one_per_operator(&with_sync, transport);
                if reachable.is_empty() {
                    return Err(NoEligibleDestinations::Empty(transport));
                }
                reachable
            }
            RotationPolicy::ExcludeSyncOperator => {
                let reachable = self.reachable(transport);
                if reachable.is_empty() {
                    return Err(NoEligibleDestinations::Empty(transport));
                }
                match sync_indexer.and_then(Operator::of_uri) {
                    None => reachable,
                    Some(sync_operator) => {
                        let eligible: Vec<Uri> = reachable
                            .into_iter()
                            .filter(|entry| {
                                Operator::of_uri(entry).as_ref() != Some(&sync_operator)
                            })
                            .collect();
                        if eligible.is_empty() {
                            return Err(NoEligibleDestinations::AllBelongToSyncOperator(
                                sync_operator,
                            ));
                        }
                        eligible
                    }
                }
            }
        };
        Ok(health.filter_with_floor(pool))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(text: &str) -> Uri {
        text.parse().expect("static uri")
    }

    fn entry(uri: &'static str, chain: IndexerChain) -> Indexer {
        Indexer {
            uri,
            chain,
            region_key: "",
            obsolete: false,
        }
    }

    fn mainnet_registry() -> Vec<Indexer> {
        vec![
            entry("https://zec.rocks:443", IndexerChain::Main),
            entry("https://eu.zec.rocks:443", IndexerChain::Main),
            entry("https://one.example:443", IndexerChain::Main),
            entry("https://two.example:9067", IndexerChain::Main),
            entry("https://three.example:443", IndexerChain::Main),
            entry("https://four.example:443", IndexerChain::Main),
            entry("https://five.example:443", IndexerChain::Main),
        ]
    }

    fn regtest() -> ChainType {
        ChainType::Regtest(crate::ActivationHeights::default())
    }

    #[test]
    fn each_chain_rules_its_policy() {
        assert_eq!(
            RotationPolicy::for_chain(&ChainType::Mainnet),
            RotationPolicy::ExcludeSyncOperator
        );
        assert_eq!(
            RotationPolicy::for_chain(&ChainType::Testnet),
            RotationPolicy::IncludeSyncIndexer
        );
        assert_eq!(
            RotationPolicy::for_chain(&regtest()),
            RotationPolicy::SyncIndexerOnly
        );
    }

    /// HYPOTHESIS: the mainnet set is the registry's live mainnet entries,
    /// one per operator, every mixnet-reachable member https on 443.
    /// Falsified if a registry entry of another chain, an obsolete entry,
    /// or a second endpoint of one operator reaches the mixnet draw.
    #[test]
    fn the_mainnet_set_is_the_registry_partitioned_and_deduplicated() {
        let set = DestinationServerSet::for_chain(&ChainType::Mainnet);
        let reachable = set.reachable(Transport::Mixnet);
        assert!(
            !reachable.is_empty(),
            "the registry carries live mainnet entries"
        );
        for entry in &reachable {
            assert_eq!(entry.scheme_str(), Some("https"), "{entry}");
            assert_eq!(entry.port_u16(), Some(MIXNET_PORT), "{entry}");
            let registered = zingo_netutils::indexers::INDEXERS
                .iter()
                .find(|indexer| indexer.uri == entry.to_string().trim_end_matches('/'))
                .expect("every member is a registry entry");
            assert_eq!(registered.chain, IndexerChain::Main, "{entry}");
            assert!(!registered.obsolete, "{entry}");
        }
        let mut operators: Vec<Operator> = reachable.iter().filter_map(Operator::of_uri).collect();
        let distinct = operators.len();
        operators.sort();
        operators.dedup();
        assert_eq!(operators.len(), distinct, "one endpoint per operator");
    }

    /// HYPOTHESIS: a testnet session draws only testnet entries. Falsified
    /// if any mainnet host reaches a testnet draw, the 2026-09-11 defect
    /// where every testnet send raced mainnet indexers.
    #[test]
    fn a_testnet_draw_never_names_a_mainnet_host() {
        let set = DestinationServerSet::for_chain(&ChainType::Testnet);
        let mainnet: Vec<String> = zingo_netutils::indexers::active(IndexerChain::Main)
            .map(|indexer| indexer.uri.to_string())
            .collect();
        for transport in [Transport::Mixnet, Transport::Clearnet] {
            let drawn = set
                .draw(transport, None, &Health::default())
                .expect("the registry carries testnet entries");
            assert!(!drawn.is_empty());
            for entry in drawn {
                assert!(
                    !mainnet.contains(&entry.to_string().trim_end_matches('/').to_string()),
                    "a testnet draw named the mainnet host {entry}"
                );
            }
        }
    }

    #[test]
    fn a_testnet_draw_keeps_the_sync_operator() {
        let set = DestinationServerSet::for_chain(&ChainType::Testnet);
        let sync = uri("https://testnet.zec.rocks:443");
        let drawn = set
            .draw(Transport::Mixnet, Some(&sync), &Health::default())
            .expect("testnet never refuses on the sync operator");
        assert!(
            drawn
                .iter()
                .any(|entry| Operator::of_uri(entry) == Operator::of_uri(&sync)),
            "testnet has one operator, so the draw must keep it: {drawn:?}"
        );
    }

    /// HYPOTHESIS: a testnet draw is the union of the registry and the
    /// sync indexer, so a private testnet indexer is a Destination beside
    /// the public one, and an empty registry leaves the sync indexer
    /// alone. Falsified if the sync indexer is missing from the union or
    /// an unreachable one is drawn.
    #[test]
    fn a_testnet_draw_unions_the_registry_with_the_sync_indexer() {
        let registry = [entry(
            "https://testnet.public.example:443",
            IndexerChain::Test,
        )];
        let set = DestinationServerSet::from_entries(RotationPolicy::IncludeSyncIndexer, &registry);
        let private = uri("https://private.testnet.example:9067");
        let drawn = set
            .draw(Transport::Clearnet, Some(&private), &Health::default())
            .expect("both remain");
        assert_eq!(
            drawn,
            vec![private.clone(), uri("https://testnet.public.example:443")]
        );
        let over_mixnet = set
            .draw(Transport::Mixnet, Some(&private), &Health::default())
            .expect("the public one remains");
        assert_eq!(over_mixnet, vec![uri("https://testnet.public.example:443")]);

        let empty = DestinationServerSet::from_entries(RotationPolicy::IncludeSyncIndexer, &[]);
        assert_eq!(
            empty.draw(Transport::Clearnet, Some(&private), &Health::default()),
            Ok(vec![private.clone()])
        );
        assert_eq!(
            empty.draw(Transport::Mixnet, Some(&private), &Health::default()),
            Err(NoEligibleDestinations::Empty(Transport::Mixnet))
        );
        assert_eq!(
            empty.draw(Transport::Clearnet, None, &Health::default()),
            Err(NoEligibleDestinations::Empty(Transport::Clearnet))
        );
    }

    #[test]
    fn a_regtest_draw_is_the_sync_indexer_alone() {
        let set = DestinationServerSet::for_chain(&regtest());
        let sync = uri("http://127.0.0.1:9067");
        assert_eq!(
            set.draw(Transport::Mixnet, Some(&sync), &Health::default()),
            Ok(vec![sync.clone()])
        );
        assert_eq!(
            set.draw(Transport::Clearnet, Some(&sync), &Health::default()),
            Ok(vec![sync])
        );
        assert_eq!(
            set.draw(Transport::Clearnet, None, &Health::default()),
            Err(NoEligibleDestinations::NoSyncIndexer)
        );
    }

    /// HYPOTHESIS: the mainnet exclusion is by operator, not exact URI,
    /// and holds on both transports (ADR 0022 as amended). Falsified if a
    /// regional variant of the sync operator survives either draw.
    #[test]
    fn the_sync_operators_regional_variant_is_excluded_on_every_transport() {
        let registry = mainnet_registry();
        let set =
            DestinationServerSet::from_entries(RotationPolicy::ExcludeSyncOperator, &registry);
        let sync = uri("https://na.zec.rocks:443");
        for transport in [Transport::Mixnet, Transport::Clearnet] {
            let drawn = set
                .draw(transport, Some(&sync), &Health::default())
                .expect("the example operators remain");
            assert!(
                drawn
                    .iter()
                    .all(|entry| Operator::of_uri(entry) != Operator::of_uri(&sync)),
                "{transport:?} drew the sync operator: {drawn:?}"
            );
        }
    }

    #[test]
    fn a_mixnet_draw_drops_non_443_members_and_a_clearnet_draw_keeps_them() {
        let registry = mainnet_registry();
        let set =
            DestinationServerSet::from_entries(RotationPolicy::ExcludeSyncOperator, &registry);
        let two = uri("https://two.example:9067");
        let mixnet = set
            .draw(Transport::Mixnet, None, &Health::default())
            .expect("members remain");
        assert!(
            !mixnet.contains(&two),
            "port 9067 never traverses the mixnet"
        );
        let clearnet = set
            .draw(Transport::Clearnet, None, &Health::default())
            .expect("members remain");
        assert!(
            clearnet.contains(&two),
            "clearnet reaches the lightwalletd port"
        );
    }

    #[test]
    fn one_endpoint_per_operator_in_registry_order() {
        let registry = mainnet_registry();
        let set =
            DestinationServerSet::from_entries(RotationPolicy::ExcludeSyncOperator, &registry);
        let reachable = set.reachable(Transport::Mixnet);
        assert_eq!(reachable[0], uri("https://zec.rocks:443"));
        assert!(!reachable.contains(&uri("https://eu.zec.rocks:443")));
    }

    #[test]
    fn a_sync_indexer_outside_the_set_excludes_nothing() {
        let registry = mainnet_registry();
        let set =
            DestinationServerSet::from_entries(RotationPolicy::ExcludeSyncOperator, &registry);
        let sync = uri("https://my.private.indexer.example:443");
        let drawn = set
            .draw(Transport::Mixnet, Some(&sync), &Health::default())
            .expect("nothing to exclude");
        assert_eq!(drawn.len(), set.reachable(Transport::Mixnet).len());
    }

    /// HYPOTHESIS: a set owned wholly by the sync operator refuses naming
    /// that operator, so a mainnet send fails closed instead of falling
    /// back to the sync indexer. Falsified if the refusal is the empty
    /// story or renders a blank operator.
    #[test]
    fn an_emptied_mainnet_draw_refuses_rather_than_drawing_the_sync_indexer() {
        let registry = [entry("https://zec.rocks:443", IndexerChain::Main)];
        let set =
            DestinationServerSet::from_entries(RotationPolicy::ExcludeSyncOperator, &registry);
        let sync = uri("https://na.zec.rocks:443");
        let err = set
            .draw(Transport::Mixnet, Some(&sync), &Health::default())
            .expect_err("the set must empty");
        assert_eq!(
            err,
            NoEligibleDestinations::AllBelongToSyncOperator(Operator::of_host("zec.rocks"))
        );
        assert!(err.to_string().contains("zec.rocks"), "{err}");
    }

    #[test]
    fn an_empty_mainnet_set_refuses_as_empty_never_as_operator_owned() {
        let set = DestinationServerSet::from_entries(RotationPolicy::ExcludeSyncOperator, &[]);
        let sync = uri("https://na.zec.rocks:443");
        for sync in [None, Some(&sync)] {
            let err = set
                .draw(Transport::Mixnet, sync, &Health::default())
                .expect_err("an empty set must refuse");
            assert_eq!(err, NoEligibleDestinations::Empty(Transport::Mixnet));
            assert!(!err.to_string().contains("operator"), "{err}");
        }
    }

    /// HYPOTHESIS: every production set is built with no trusted server,
    /// so the trusted branch is dormant until configuration feeds it
    /// (ruling 2026-09-12). Falsified if any chain's set carries one.
    #[test]
    fn production_sets_carry_no_trusted_server() {
        for chain in [ChainType::Mainnet, ChainType::Testnet, regtest()] {
            assert!(
                DestinationServerSet::for_chain(&chain).trusted().is_empty(),
                "{chain:?} must build an empty trusted set"
            );
        }
    }

    /// HYPOTHESIS: a reachable trusted server is the sole Destination,
    /// with no exclusion and no rotation, even when it is the sync
    /// indexer on mainnet. Falsified if the draw names any registry member
    /// or refuses the sync operator.
    #[test]
    fn a_reachable_trusted_server_is_drawn_alone() {
        let registry = mainnet_registry();
        let own = uri("https://node.mine.example:443");
        let set =
            DestinationServerSet::from_entries(RotationPolicy::ExcludeSyncOperator, &registry)
                .with_trusted([own.clone()]);
        for transport in [Transport::Mixnet, Transport::Clearnet] {
            assert_eq!(
                set.draw(transport, Some(&own), &Health::default()),
                Ok(vec![own.clone()]),
                "{transport:?}"
            );
        }
    }

    /// HYPOTHESIS: a trusted server the wire cannot reach falls through
    /// to the untrusted draw on that wire only. Falsified if a LAN node
    /// is drawn over the mixnet, or if the clearnet draw leaves it for the
    /// registry.
    #[test]
    fn an_unreachable_trusted_server_falls_through_on_that_wire_only() {
        let registry = mainnet_registry();
        let lan = uri("http://192.168.1.10:9067");
        let set =
            DestinationServerSet::from_entries(RotationPolicy::ExcludeSyncOperator, &registry)
                .with_trusted([lan.clone()]);
        assert_eq!(
            set.draw(Transport::Clearnet, Some(&lan), &Health::default()),
            Ok(vec![lan.clone()])
        );
        let over_mixnet = set
            .draw(Transport::Mixnet, Some(&lan), &Health::default())
            .expect("the registry carries the mixnet draw");
        assert!(!over_mixnet.contains(&lan));
        assert_eq!(over_mixnet, set.reachable(Transport::Mixnet));
    }

    /// The trusted servers are one per operator too: two names for one
    /// node are one Destination.
    #[test]
    fn trusted_servers_are_one_per_operator() {
        let set = DestinationServerSet::from_uris(RotationPolicy::ExcludeSyncOperator, [])
            .with_trusted([
                uri("https://a.mine.example:443"),
                uri("https://b.mine.example:443"),
            ]);
        assert_eq!(
            set.draw(Transport::Clearnet, None, &Health::default()),
            Ok(vec![uri("https://a.mine.example:443")])
        );
    }
}

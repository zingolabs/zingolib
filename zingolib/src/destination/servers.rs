//! The indexers a session may broadcast to, and the rule that draws from them
//! (ADR 0050).

use http::Uri;
use zingo_netutils::indexers::{Indexer, IndexerChain};

use super::Operator;
use super::health::Health;
use crate::config::ChainType;

const MIXNET_PORT: u16 = 443;

const HTTP_DEFAULT_PORT: u16 = 80;

/// The wire a broadcast travels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// A direct connection.
    Clearnet,
    /// The mixnet's SOCKS5 tunnel.
    Mixnet,
}

/// Whether an indexer's operator may link the wallet to its transactions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trust {
    /// It may.
    Trusted,
    /// It may not.
    Untrusted,
}

impl Trust {
    /// The trust `chain` gives an unclassified remote indexer.
    pub fn remote_default(chain: &ChainType) -> Self {
        match chain {
            ChainType::Mainnet => Trust::Untrusted,
            ChainType::Testnet | ChainType::Regtest(_) => Trust::Trusted,
        }
    }
}

/// What an indexer is used for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Sync only.
    Sync,
    /// Broadcast only.
    Broadcast,
    /// Both.
    SyncAndBroadcast,
}

impl Role {
    fn broadcasts(self) -> bool {
        matches!(self, Role::Broadcast | Role::SyncAndBroadcast)
    }
}

/// Where an indexer runs, relative to the wallet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Location {
    /// On the wallet's machine or local network.
    Local,
    /// Anywhere else.
    Remote,
}

impl Location {
    /// The location `uri`'s host names.
    pub fn of_uri(uri: &Uri) -> Self {
        let Some(host) = uri.host() else {
            return Location::Remote;
        };
        let host = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim_end_matches('.');
        let lowered = host.to_ascii_lowercase();
        let local_name = lowered == "localhost"
            || lowered.ends_with(".localhost")
            || lowered.ends_with(".local");
        let address = host
            .parse::<std::net::IpAddr>()
            .map(|address| address.to_canonical());
        let local_address = match address {
            Ok(std::net::IpAddr::V4(address)) => {
                address.is_loopback()
                    || address.is_private()
                    || address.is_link_local()
                    || address.is_unspecified()
            }
            Ok(std::net::IpAddr::V6(address)) => {
                address.is_loopback()
                    || address.is_unspecified()
                    || address.is_unique_local()
                    || address.is_unicast_link_local()
            }
            Err(_) => false,
        };
        if local_name || local_address {
            Location::Local
        } else {
            Location::Remote
        }
    }
}

/// A consumer's classification of one indexer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexerConfig {
    uri: Uri,
    role: Option<Role>,
    trust: Option<Trust>,
    #[cfg(any(test, feature = "testutils"))]
    location: Option<Location>,
}

impl IndexerConfig {
    /// The indexer at `uri`, unclassified.
    pub fn new(uri: Uri) -> Self {
        IndexerConfig {
            uri,
            role: None,
            trust: None,
            #[cfg(any(test, feature = "testutils"))]
            location: None,
        }
    }

    /// Sets its role.
    #[must_use]
    pub fn role(mut self, role: Role) -> Self {
        self.role = Some(role);
        self
    }

    /// Sets its trust.
    #[must_use]
    pub fn trust(mut self, trust: Trust) -> Self {
        self.trust = Some(trust);
        self
    }

    /// Overrides the location its URI implies.
    #[cfg(any(test, feature = "testutils"))]
    #[must_use]
    pub fn location(mut self, location: Location) -> Self {
        self.location = Some(location);
        self
    }

    /// Where the indexer is addressed.
    pub fn uri(&self) -> &Uri {
        &self.uri
    }

    fn location_override(&self) -> Option<Location> {
        #[cfg(any(test, feature = "testutils"))]
        {
            self.location
        }
        #[cfg(not(any(test, feature = "testutils")))]
        {
            None
        }
    }
}

fn same_endpoint(a: &Uri, b: &Uri) -> bool {
    let port = |uri: &Uri| {
        uri.port_u16().unwrap_or(match uri.scheme_str() {
            Some("https") => MIXNET_PORT,
            _ => HTTP_DEFAULT_PORT,
        })
    };
    a.host()
        .zip(b.host())
        .is_some_and(|(a_host, b_host)| a_host.eq_ignore_ascii_case(b_host))
        && port(a) == port(b)
}

fn registry_chain(chain: &ChainType) -> Option<IndexerChain> {
    match chain {
        ChainType::Mainnet => Some(IndexerChain::Main),
        ChainType::Testnet => Some(IndexerChain::Test),
        ChainType::Regtest(_) => None,
    }
}

#[derive(Clone, Debug)]
struct Classified {
    uri: Uri,
    operator: Operator,
    trust: Trust,
    role: Role,
    location: Location,
}

impl Classified {
    fn reachable_over(&self, transport: Transport) -> bool {
        match transport {
            Transport::Clearnet => true,
            Transport::Mixnet => {
                self.location == Location::Remote
                    && self.uri.scheme_str() == Some("https")
                    && self.uri.port_u16().unwrap_or(MIXNET_PORT) == MIXNET_PORT
            }
        }
    }
}

/// Why a broadcast has no Destination.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NoEligibleDestinations {
    /// Every reachable entry belongs to the untrusted sync indexer's operator.
    #[error(
        "no eligible Destination: every reachable entry belongs to the \
         untrusted sync indexer's operator ({0}), and a mixnet broadcast \
         never goes to it"
    )]
    AllBelongToSyncOperator(Operator),
    /// Nothing is reachable over the transport.
    #[error("no eligible Destination: no configured or registry indexer is reachable over {0:?}")]
    Empty(Transport),
}

/// The ordered Destinations one broadcast may contact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draw {
    destinations: Vec<Uri>,
    preferred: usize,
}

impl Draw {
    /// Every Destination the race may contact.
    pub fn destinations(&self) -> &[Uri] {
        &self.destinations
    }

    /// How many leading Destinations keep their order.
    pub fn preferred(&self) -> usize {
        self.preferred
    }
}

/// The indexers a session may broadcast to.
#[derive(Clone, Debug)]
pub struct DestinationServerSet {
    remote_trust: Trust,
    configured: Vec<IndexerConfig>,
    registry: Vec<(Uri, Operator)>,
}

impl DestinationServerSet {
    /// The set for `chain`, with the consumer's classifications and remote trust.
    pub fn for_chain(
        chain: &ChainType,
        remote_trust: Option<Trust>,
        configured: Vec<IndexerConfig>,
    ) -> Self {
        let entries = registry_chain(chain)
            .into_iter()
            .flat_map(zingo_netutils::indexers::active);
        let mut set = Self::from_entries(
            remote_trust.unwrap_or_else(|| Trust::remote_default(chain)),
            entries,
        );
        configured
            .into_iter()
            .for_each(|config| set.add_indexer(config));
        set
    }

    pub(crate) fn from_entries<'a>(
        remote_trust: Trust,
        entries: impl IntoIterator<Item = &'a Indexer>,
    ) -> Self {
        Self::from_uris(
            remote_trust,
            entries
                .into_iter()
                .filter_map(|entry| entry.uri.parse::<Uri>().ok()),
        )
    }

    /// A set whose registry is `uris`.
    pub fn from_uris(remote_trust: Trust, uris: impl IntoIterator<Item = Uri>) -> Self {
        DestinationServerSet {
            remote_trust,
            configured: Vec::new(),
            registry: uris
                .into_iter()
                .filter_map(|uri| Operator::of_uri(&uri).map(|operator| (uri, operator)))
                .collect(),
        }
    }

    /// A set whose registry names each entry's operator.
    #[cfg(any(test, feature = "testutils"))]
    pub fn registry_for_tests<'a>(
        remote_trust: Trust,
        entries: impl IntoIterator<Item = (Uri, &'a str)>,
    ) -> Self {
        DestinationServerSet {
            remote_trust,
            configured: Vec::new(),
            registry: entries
                .into_iter()
                .map(|(uri, operator)| (uri, Operator::of_host(operator)))
                .collect(),
        }
    }

    /// The set with `config` added.
    #[must_use]
    pub fn with_indexer(mut self, config: IndexerConfig) -> Self {
        self.add_indexer(config);
        self
    }

    /// Adds or replaces the classification of `config`'s endpoint.
    pub fn add_indexer(&mut self, config: IndexerConfig) {
        match self
            .configured
            .iter_mut()
            .find(|existing| same_endpoint(&existing.uri, &config.uri))
        {
            Some(existing) => *existing = config,
            None => self.configured.push(config),
        }
    }

    /// The trust an unclassified remote indexer receives.
    pub fn remote_trust(&self) -> Trust {
        self.remote_trust
    }

    /// The trust `uri` resolves to.
    pub fn trust_of(&self, uri: &Uri) -> Trust {
        self.resolve(uri, self.config_for(uri))
    }

    fn config_for(&self, uri: &Uri) -> Option<&IndexerConfig> {
        self.configured
            .iter()
            .find(|config| same_endpoint(&config.uri, uri))
    }

    fn resolve(&self, uri: &Uri, config: Option<&IndexerConfig>) -> Trust {
        let location = config
            .and_then(IndexerConfig::location_override)
            .unwrap_or_else(|| Location::of_uri(uri));
        config
            .and_then(|config| config.trust)
            .unwrap_or(match location {
                Location::Local => Trust::Trusted,
                Location::Remote => self.remote_trust,
            })
    }

    fn classify(&self, uri: &Uri) -> Option<Classified> {
        let config = self.config_for(uri);
        Some(Classified {
            uri: uri.clone(),
            operator: Operator::of_uri(uri)?,
            trust: self.resolve(uri, config),
            role: config
                .and_then(|config| config.role)
                .unwrap_or(Role::SyncAndBroadcast),
            location: config
                .and_then(IndexerConfig::location_override)
                .unwrap_or_else(|| Location::of_uri(uri)),
        })
    }

    /// The registry entries reachable over `transport`, one per operator.
    pub fn registry_reachable(&self, transport: Transport) -> Vec<Uri> {
        let mut seen: Vec<&Operator> = Vec::new();
        self.registry
            .iter()
            .filter(|(uri, _)| {
                transport == Transport::Clearnet
                    || uri.scheme_str() == Some("https")
                        && uri.port_u16().unwrap_or(MIXNET_PORT) == MIXNET_PORT
            })
            .filter(|(_, operator)| {
                if seen.contains(&operator) {
                    false
                } else {
                    seen.push(operator);
                    true
                }
            })
            .map(|(uri, _)| uri.clone())
            .collect()
    }

    /// The Destinations one broadcast over `transport` may contact.
    pub fn draw(
        &self,
        transport: Transport,
        sync_indexer: Option<&Uri>,
        health: &Health,
    ) -> Result<Draw, NoEligibleDestinations> {
        self.draw_reaching(transport, transport, sync_indexer, health)
    }

    /// [`Self::draw`] with reachability judged over `reach`.
    pub(crate) fn draw_reaching(
        &self,
        transport: Transport,
        reach: Transport,
        sync_indexer: Option<&Uri>,
        health: &Health,
    ) -> Result<Draw, NoEligibleDestinations> {
        let sync = sync_indexer.and_then(|uri| self.classify(uri));
        let excluded_operator = sync
            .as_ref()
            .filter(|sync| transport == Transport::Mixnet && sync.trust == Trust::Untrusted)
            .map(|sync| sync.operator.clone());
        let excluded = |candidate: &Classified| {
            candidate.trust == Trust::Untrusted
                && excluded_operator.as_ref() == Some(&candidate.operator)
        };

        let configured = self
            .configured
            .iter()
            .filter(|config| {
                sync_indexer.is_none_or(|sync_uri| !same_endpoint(&config.uri, sync_uri))
            })
            .filter_map(|config| self.classify(&config.uri))
            .filter(|candidate| candidate.role.broadcasts() && candidate.reachable_over(reach));
        let sync_candidate = sync
            .clone()
            .filter(|sync| sync.role.broadcasts() && sync.reachable_over(reach));
        let registry: Vec<Classified> = if transport == Transport::Mixnet {
            self.registry
                .iter()
                .map(|(uri, operator)| Classified {
                    uri: uri.clone(),
                    operator: operator.clone(),
                    trust: self.remote_trust,
                    role: Role::Broadcast,
                    location: Location::Remote,
                })
                .filter(|candidate| candidate.reachable_over(reach))
                .collect()
        } else {
            Vec::new()
        };

        let mut dropped_by_exclusion = false;
        let mut trusted: Vec<Uri> = Vec::new();
        let mut untrusted: Vec<(Classified, bool)> = Vec::new();
        let tiers = configured
            .map(|candidate| (candidate, true))
            .chain(sync_candidate.map(|candidate| (candidate, false)))
            .chain(registry.into_iter().map(|candidate| (candidate, false)));
        for (candidate, is_configured) in tiers {
            match candidate.trust {
                Trust::Trusted => {
                    if !trusted
                        .iter()
                        .any(|kept| same_endpoint(kept, &candidate.uri))
                    {
                        trusted.push(candidate.uri);
                    }
                }
                Trust::Untrusted if excluded(&candidate) => dropped_by_exclusion = true,
                Trust::Untrusted => untrusted.push((candidate, is_configured)),
            }
        }

        let mut seen: Vec<Operator> = Vec::new();
        let mut preferred = Vec::new();
        let mut rest = Vec::new();
        for (candidate, is_configured) in untrusted {
            if seen.contains(&candidate.operator) {
                continue;
            }
            seen.push(candidate.operator);
            if is_configured {
                preferred.push(candidate.uri);
            } else {
                rest.push(candidate.uri);
            }
        }

        if !trusted.is_empty() {
            return Ok(Draw {
                destinations: trusted,
                preferred: 0,
            });
        }
        let preferred_len = preferred.len();
        let mut destinations = preferred;
        destinations.extend(health.filter_with_floor(rest));
        if destinations.is_empty() {
            return Err(match excluded_operator {
                Some(operator) if dropped_by_exclusion => {
                    NoEligibleDestinations::AllBelongToSyncOperator(operator)
                }
                _ => NoEligibleDestinations::Empty(transport),
            });
        }
        Ok(Draw {
            destinations,
            preferred: preferred_len,
        })
    }
}

#[cfg(test)]
mod draw_exhaustive;

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

    fn mainnet_set() -> DestinationServerSet {
        DestinationServerSet::from_entries(Trust::Untrusted, &mainnet_registry())
    }

    fn regtest() -> ChainType {
        ChainType::Regtest(crate::ActivationHeights::default())
    }

    fn drawn(set: &DestinationServerSet, transport: Transport, sync: Option<&Uri>) -> Vec<Uri> {
        set.draw(transport, sync, &Health::default())
            .expect("the draw has candidates")
            .destinations()
            .to_vec()
    }

    #[test]
    fn locations_follow_the_address() {
        let table = [
            ("http://127.0.0.1:9067", Location::Local),
            ("http://127.255.255.255:9067", Location::Local),
            ("http://126.255.255.255:9067", Location::Remote),
            ("http://128.0.0.0:9067", Location::Remote),
            ("http://10.0.0.0:9067", Location::Local),
            ("http://10.255.255.255:9067", Location::Local),
            ("http://11.0.0.0:9067", Location::Remote),
            ("http://172.15.255.255:9067", Location::Remote),
            ("http://172.16.0.0:9067", Location::Local),
            ("http://172.31.255.255:9067", Location::Local),
            ("http://172.32.0.0:9067", Location::Remote),
            ("http://192.167.255.255:9067", Location::Remote),
            ("http://192.168.0.0:9067", Location::Local),
            ("http://192.168.255.255:9067", Location::Local),
            ("http://192.169.0.0:9067", Location::Remote),
            ("http://169.254.0.0:9067", Location::Local),
            ("http://169.254.255.255:9067", Location::Local),
            ("http://169.253.255.255:9067", Location::Remote),
            ("http://0.0.0.0:9067", Location::Local),
            ("http://100.64.0.1:9067", Location::Remote),
            ("https://8.8.8.8:443", Location::Remote),
            ("http://[::1]:9067", Location::Local),
            ("http://[::]:9067", Location::Local),
            ("http://[fbff:ffff::1]:9067", Location::Remote),
            ("http://[fc00::1]:9067", Location::Local),
            ("http://[fdff:ffff::1]:9067", Location::Local),
            ("http://[fe00::1]:9067", Location::Remote),
            ("http://[fe80::1]:9067", Location::Local),
            ("http://[febf:ffff::1]:9067", Location::Local),
            ("http://[fec0::1]:9067", Location::Remote),
            ("http://[::ffff:127.0.0.1]:9067", Location::Local),
            ("http://[::ffff:192.168.1.1]:9067", Location::Local),
            ("http://[::ffff:8.8.8.8]:9067", Location::Remote),
            ("https://[2001:db8::1]:443", Location::Remote),
            ("http://localhost:9067", Location::Local),
            ("http://LOCALHOST:9067", Location::Local),
            ("http://localhost.:9067", Location::Local),
            ("http://api.localhost:9067", Location::Local),
            ("http://node.local:9067", Location::Local),
            ("http://node.LOCAL.:9067", Location::Local),
            ("https://localhost.evil.example:443", Location::Remote),
            ("https://evil-localhost.example:443", Location::Remote),
            ("https://local.example:443", Location::Remote),
            ("https://nodelocal:443", Location::Remote),
            ("https://127.0.0.1.nip.io:443", Location::Remote),
            ("https://zec.rocks:443", Location::Remote),
            ("/no-host", Location::Remote),
        ];
        for (text, expected) in table {
            assert_eq!(Location::of_uri(&uri(text)), expected, "{text}");
        }
    }

    #[test]
    fn endpoints_match_by_host_and_port() {
        let table = [
            ("https://node.example:443", "https://NODE.example", true),
            ("http://node.example:80", "http://node.example", true),
            (
                "https://node.example:443",
                "https://node.example:9067",
                false,
            ),
            ("https://node.example", "http://node.example", false),
            ("https://a.example:443", "https://b.example:443", false),
            ("http://[::1]:9067", "http://[::1]:9067", true),
            ("/no-host", "/no-host", false),
        ];
        for (a, b, expected) in table {
            assert_eq!(same_endpoint(&uri(a), &uri(b)), expected, "{a} vs {b}");
        }
    }

    #[test]
    fn trust_resolves_explicit_then_location_then_default() {
        let local = uri("http://192.168.1.10:9067");
        let remote = uri("https://node.example:443");
        for remote_trust in [Trust::Trusted, Trust::Untrusted] {
            let bare = DestinationServerSet::from_uris(remote_trust, []);
            assert_eq!(bare.trust_of(&local), Trust::Trusted);
            assert_eq!(bare.trust_of(&remote), remote_trust);
            for explicit in [Trust::Trusted, Trust::Untrusted] {
                let set = DestinationServerSet::from_uris(remote_trust, [])
                    .with_indexer(IndexerConfig::new(local.clone()).trust(explicit))
                    .with_indexer(IndexerConfig::new(remote.clone()).trust(explicit));
                assert_eq!(set.trust_of(&local), explicit);
                assert_eq!(set.trust_of(&remote), explicit);
            }
        }
        let set = DestinationServerSet::from_uris(Trust::Untrusted, [])
            .with_indexer(IndexerConfig::new(local.clone()).location(Location::Remote));
        assert_eq!(
            set.trust_of(&local),
            Trust::Untrusted,
            "a location override feeds the default"
        );
    }

    #[test]
    fn each_chain_reads_its_own_registry_partition() {
        for (chain, own, other) in [
            (ChainType::Mainnet, IndexerChain::Main, IndexerChain::Test),
            (ChainType::Testnet, IndexerChain::Test, IndexerChain::Main),
        ] {
            let set = DestinationServerSet::for_chain(&chain, None, Vec::new());
            let held: Vec<String> = set
                .registry_reachable(Transport::Clearnet)
                .iter()
                .map(|entry| entry.to_string().trim_end_matches('/').to_string())
                .collect();
            assert!(!held.is_empty(), "{chain:?}");
            for entry in &held {
                assert!(
                    zingo_netutils::indexers::active(own).any(|indexer| indexer.uri == entry),
                    "{chain:?} holds {entry}, which is not an active entry of its chain"
                );
                assert!(
                    !zingo_netutils::indexers::INDEXERS
                        .iter()
                        .any(|indexer| indexer.chain == other && indexer.uri == entry),
                    "{chain:?} holds {entry} from the other chain"
                );
            }
        }
        let regtest = DestinationServerSet::for_chain(&regtest(), None, Vec::new());
        assert!(regtest.registry_reachable(Transport::Clearnet).is_empty());
    }

    #[test]
    fn for_chain_takes_the_consumer_override_and_classifications() {
        let own = uri("https://node.mine.example:443");
        let overridden =
            DestinationServerSet::for_chain(&ChainType::Mainnet, Some(Trust::Trusted), Vec::new());
        assert_eq!(overridden.remote_trust(), Trust::Trusted);
        let classified = DestinationServerSet::for_chain(
            &ChainType::Mainnet,
            None,
            vec![IndexerConfig::new(own.clone()).trust(Trust::Trusted)],
        );
        assert_eq!(classified.remote_trust(), Trust::Untrusted);
        assert_eq!(classified.trust_of(&own), Trust::Trusted);
    }

    #[test]
    fn each_chain_rules_its_remote_default() {
        assert_eq!(Trust::remote_default(&ChainType::Mainnet), Trust::Untrusted);
        assert_eq!(Trust::remote_default(&ChainType::Testnet), Trust::Trusted);
        assert_eq!(Trust::remote_default(&regtest()), Trust::Trusted);
    }

    #[test]
    fn the_mainnet_registry_is_partitioned_and_deduplicated() {
        let set = DestinationServerSet::for_chain(&ChainType::Mainnet, None, Vec::new());
        let reachable = set.registry_reachable(Transport::Mixnet);
        assert!(!reachable.is_empty());
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

    #[test]
    fn a_testnet_draw_never_names_a_mainnet_host() {
        let set = DestinationServerSet::for_chain(&ChainType::Testnet, None, Vec::new());
        let mainnet: Vec<String> = zingo_netutils::indexers::active(IndexerChain::Main)
            .map(|indexer| indexer.uri.to_string())
            .collect();
        let destinations = drawn(&set, Transport::Mixnet, None);
        assert!(!destinations.is_empty());
        for entry in destinations {
            assert!(
                !mainnet.contains(&entry.to_string().trim_end_matches('/').to_string()),
                "a testnet draw named the mainnet host {entry}"
            );
        }
    }

    #[test]
    fn a_clearnet_draw_never_rotates_across_the_registry() {
        let sync = uri("https://zec.rocks:443");
        assert_eq!(
            drawn(&mainnet_set(), Transport::Clearnet, Some(&sync)),
            vec![sync]
        );
        assert_eq!(
            mainnet_set().draw(Transport::Clearnet, None, &Health::default()),
            Err(NoEligibleDestinations::Empty(Transport::Clearnet))
        );
    }

    #[test]
    fn a_mixnet_draw_excludes_the_untrusted_sync_operator() {
        let sync = uri("https://na.zec.rocks:443");
        let destinations = drawn(&mainnet_set(), Transport::Mixnet, Some(&sync));
        assert!(!destinations.is_empty());
        assert!(
            destinations
                .iter()
                .all(|entry| Operator::of_uri(entry) != Operator::of_uri(&sync)),
            "{destinations:?}"
        );
    }

    #[test]
    fn a_mixnet_draw_keeps_only_port_443() {
        let destinations = drawn(&mainnet_set(), Transport::Mixnet, None);
        assert!(!destinations.contains(&uri("https://two.example:9067")));
        assert!(destinations.contains(&uri("https://one.example:443")));
    }

    #[test]
    fn a_local_sync_indexer_takes_clearnet_and_yields_the_mixnet() {
        let local = uri("http://192.168.1.10:9067");
        assert_eq!(
            drawn(&mainnet_set(), Transport::Clearnet, Some(&local)),
            vec![local.clone()]
        );
        let over_mixnet = drawn(&mainnet_set(), Transport::Mixnet, Some(&local));
        assert!(!over_mixnet.contains(&local));
        assert_eq!(
            over_mixnet.len(),
            mainnet_set().registry_reachable(Transport::Mixnet).len(),
            "a trusted local node excludes no registry operator"
        );
    }

    #[test]
    fn a_local_https_indexer_is_unreachable_over_the_mixnet() {
        let local = uri("https://127.0.0.1:443");
        let set = DestinationServerSet::from_uris(Trust::Trusted, []);
        assert_eq!(
            set.draw(Transport::Mixnet, Some(&local), &Health::default()),
            Err(NoEligibleDestinations::Empty(Transport::Mixnet))
        );
    }

    #[test]
    fn a_trusted_remote_sync_indexer_is_drawn_alone() {
        let own = uri("https://node.mine.example:443");
        let set = mainnet_set().with_indexer(IndexerConfig::new(own.clone()).trust(Trust::Trusted));
        for transport in [Transport::Mixnet, Transport::Clearnet] {
            assert_eq!(drawn(&set, transport, Some(&own)), vec![own.clone()]);
        }
    }

    #[test]
    fn a_trusted_broadcast_indexer_is_drawn_alone() {
        let vps = uri("https://vps.mine.example:443");
        let sync = uri("https://zec.rocks:443");
        let set = mainnet_set().with_indexer(
            IndexerConfig::new(vps.clone())
                .role(Role::Broadcast)
                .trust(Trust::Trusted),
        );
        for transport in [Transport::Mixnet, Transport::Clearnet] {
            assert_eq!(drawn(&set, transport, Some(&sync)), vec![vps.clone()]);
        }
    }

    #[test]
    fn a_configured_untrusted_broadcast_indexer_leads_the_race() {
        let own = uri("https://relay.example:443");
        let sync = uri("https://zec.rocks:443");
        let set = mainnet_set().with_indexer(IndexerConfig::new(own.clone()).role(Role::Broadcast));
        let clearnet = set
            .draw(Transport::Clearnet, Some(&sync), &Health::default())
            .expect("two candidates");
        assert_eq!(clearnet.destinations(), &[own.clone(), sync.clone()]);
        assert_eq!(clearnet.preferred(), 1);
        let mixnet = set
            .draw(Transport::Mixnet, Some(&sync), &Health::default())
            .expect("the relay and the registry");
        assert_eq!(mixnet.destinations()[0], own);
        assert_eq!(mixnet.preferred(), 1);
        assert!(!mixnet.destinations().contains(&sync));
    }

    #[test]
    fn a_sync_only_indexer_never_receives_a_broadcast() {
        let sync = uri("https://zec.rocks:443");
        let set = mainnet_set().with_indexer(IndexerConfig::new(sync.clone()).role(Role::Sync));
        assert_eq!(
            set.draw(Transport::Clearnet, Some(&sync), &Health::default()),
            Err(NoEligibleDestinations::Empty(Transport::Clearnet))
        );
    }

    #[test]
    fn a_testnet_remote_sync_indexer_is_trusted_by_default() {
        let set = DestinationServerSet::for_chain(&ChainType::Testnet, None, Vec::new());
        let sync = uri("https://testnet.zec.rocks:443");
        for transport in [Transport::Mixnet, Transport::Clearnet] {
            assert!(drawn(&set, transport, Some(&sync)).contains(&sync));
        }
    }

    #[test]
    fn a_regtest_draw_reaches_its_local_sync_indexer_over_clearnet_only() {
        let set = DestinationServerSet::for_chain(&regtest(), None, Vec::new());
        let sync = uri("http://127.0.0.1:9067");
        assert_eq!(
            drawn(&set, Transport::Clearnet, Some(&sync)),
            vec![sync.clone()]
        );
        assert_eq!(
            set.draw(Transport::Mixnet, Some(&sync), &Health::default()),
            Err(NoEligibleDestinations::Empty(Transport::Mixnet))
        );
    }

    #[test]
    fn an_emptied_mixnet_draw_refuses_rather_than_drawing_the_sync_indexer() {
        let set = DestinationServerSet::from_entries(
            Trust::Untrusted,
            &[entry("https://zec.rocks:443", IndexerChain::Main)],
        );
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
    fn a_configured_classification_applies_to_the_same_endpoint_only() {
        let own = uri("https://Node.Mine.Example:443");
        let set = mainnet_set().with_indexer(IndexerConfig::new(own).trust(Trust::Trusted));
        assert_eq!(
            set.trust_of(&uri("https://node.mine.example")),
            Trust::Trusted
        );
        assert_eq!(
            set.trust_of(&uri("https://node.mine.example:9067")),
            Trust::Untrusted
        );
    }
}

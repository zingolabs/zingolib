//! The broadcast rule, checked over every combination of its inputs.

use std::collections::BTreeSet;

use super::*;

const LIGHTWALLETD_PORT: u16 = 9067;

const SYNC_OPERATOR: &str = "sync.example";
const RELAY_OPERATOR: &str = "relay.example";
const SHARED_OPERATOR: &str = "shared.example";
const OTHER_OPERATOR: &str = "other.example";
const FAR_OPERATOR: &str = "far.example";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Configured,
    Sync,
    Registry,
}

#[derive(Clone, Copy, Debug)]
struct Shape {
    location: Location,
    port: u16,
    trust: Option<Trust>,
    role: Option<Role>,
}

#[derive(Clone, Debug)]
struct Endpoint {
    uri: Uri,
    source: Source,
    location: Location,
    trust: Trust,
    role: Role,
    operator: Operator,
}

#[derive(Clone, Debug)]
struct Case {
    transport: Transport,
    remote_trust: Trust,
    sync: Option<(Shape, Endpoint)>,
    configured: Option<(Shape, Endpoint)>,
    registry: Vec<Endpoint>,
}

fn remote_uri(label: &str, operator: &str, port: u16) -> Uri {
    format!("https://{label}.{operator}:{port}")
        .parse()
        .expect("a static uri")
}

fn local_uri(slot: u8, port: u16) -> Uri {
    format!("https://10.0.{slot}.1:{port}")
        .parse()
        .expect("a static uri")
}

fn resolve(uri: Uri, source: Source, shape: Shape, remote_trust: Trust) -> Endpoint {
    Endpoint {
        operator: Operator::of_uri(&uri).expect("every case uri has a host"),
        uri,
        source,
        location: shape.location,
        trust: shape.trust.unwrap_or(match shape.location {
            Location::Local => Trust::Trusted,
            Location::Remote => remote_trust,
        }),
        role: shape.role.unwrap_or(Role::SyncAndBroadcast),
    }
}

fn registry_variants(remote_trust: Trust) -> Vec<Vec<Endpoint>> {
    let entry = |label: &str, operator: &str, port: u16| Endpoint {
        uri: remote_uri(label, operator, port),
        source: Source::Registry,
        location: Location::Remote,
        trust: remote_trust,
        role: Role::Broadcast,
        operator: Operator::of_host(operator),
    };
    vec![
        Vec::new(),
        vec![entry("a", SHARED_OPERATOR, MIXNET_PORT)],
        vec![
            entry("a", SHARED_OPERATOR, MIXNET_PORT),
            entry("b", OTHER_OPERATOR, MIXNET_PORT),
            entry("c", FAR_OPERATOR, LIGHTWALLETD_PORT),
            entry("d", OTHER_OPERATOR, MIXNET_PORT),
        ],
    ]
}

fn shapes() -> Vec<Shape> {
    let mut shapes = Vec::new();
    for location in [Location::Local, Location::Remote] {
        for port in [MIXNET_PORT, LIGHTWALLETD_PORT] {
            for trust in [None, Some(Trust::Trusted), Some(Trust::Untrusted)] {
                for role in [
                    None,
                    Some(Role::Sync),
                    Some(Role::Broadcast),
                    Some(Role::SyncAndBroadcast),
                ] {
                    shapes.push(Shape {
                        location,
                        port,
                        trust,
                        role,
                    });
                }
            }
        }
    }
    shapes
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for transport in [Transport::Clearnet, Transport::Mixnet] {
        for remote_trust in [Trust::Trusted, Trust::Untrusted] {
            for registry in registry_variants(remote_trust) {
                let mut syncs: Vec<Option<(Shape, Endpoint)>> = vec![None];
                for shape in shapes() {
                    let uris = match shape.location {
                        Location::Local => vec![local_uri(1, shape.port)],
                        Location::Remote => vec![
                            remote_uri("node", SYNC_OPERATOR, shape.port),
                            remote_uri("node", SHARED_OPERATOR, shape.port),
                        ],
                    };
                    for uri in uris {
                        syncs.push(Some((
                            shape,
                            resolve(uri, Source::Sync, shape, remote_trust),
                        )));
                    }
                }
                for sync in &syncs {
                    let mut configureds: Vec<Option<(Shape, Endpoint)>> = vec![None];
                    for shape in shapes() {
                        let mut uris = match shape.location {
                            Location::Local => vec![local_uri(2, shape.port)],
                            Location::Remote => vec![
                                remote_uri("relay", RELAY_OPERATOR, shape.port),
                                remote_uri("relay", SHARED_OPERATOR, shape.port),
                            ],
                        };
                        if let (Some((_, sync)), Location::Remote) = (sync, shape.location)
                            && sync.location == Location::Remote
                        {
                            uris.push(remote_uri("relay", &sync.operator.to_string(), shape.port));
                        }
                        for uri in uris {
                            configureds.push(Some((
                                shape,
                                resolve(uri, Source::Configured, shape, remote_trust),
                            )));
                        }
                    }
                    for configured in configureds {
                        cases.push(Case {
                            transport,
                            remote_trust,
                            sync: sync.clone(),
                            configured,
                            registry: registry.clone(),
                        });
                    }
                }
            }
        }
    }
    cases
}

fn config_for(shape: Shape, endpoint: &Endpoint) -> IndexerConfig {
    let mut config = IndexerConfig::new(endpoint.uri.clone());
    if let Some(trust) = shape.trust {
        config = config.trust(trust);
    }
    if let Some(role) = shape.role {
        config = config.role(role);
    }
    config
}

fn build(case: &Case) -> DestinationServerSet {
    let mut set = DestinationServerSet::from_uris(
        case.remote_trust,
        case.registry.iter().map(|entry| entry.uri.clone()),
    );
    if let Some((shape, sync)) = &case.sync
        && (shape.trust.is_some() || shape.role.is_some())
    {
        set.add_indexer(config_for(*shape, sync));
    }
    if let Some((shape, configured)) = &case.configured {
        set.add_indexer(config_for(*shape, configured));
    }
    set
}

fn reachable(endpoint: &Endpoint, transport: Transport) -> bool {
    match transport {
        Transport::Clearnet => true,
        Transport::Mixnet => {
            endpoint.location == Location::Remote
                && endpoint.uri.scheme_str() == Some("https")
                && endpoint.uri.port_u16() == Some(MIXNET_PORT)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Expected {
    Trusted(BTreeSet<String>),
    Untrusted {
        preferred: Vec<String>,
        rest: BTreeSet<String>,
    },
    Refused(NoEligibleDestinations),
}

fn oracle(case: &Case) -> Expected {
    let sync = case.sync.as_ref().map(|(_, sync)| sync);
    let excluded_operator = sync
        .filter(|sync| case.transport == Transport::Mixnet && sync.trust == Trust::Untrusted)
        .map(|sync| sync.operator.clone());

    let everyone = case
        .configured
        .iter()
        .map(|(_, configured)| configured)
        .chain(sync)
        .chain(&case.registry);
    let candidates: Vec<&Endpoint> = everyone
        .filter(|endpoint| endpoint.role.broadcasts())
        .filter(|endpoint| reachable(endpoint, case.transport))
        .filter(|endpoint| {
            endpoint.source != Source::Registry || case.transport == Transport::Mixnet
        })
        .collect();

    let trusted: BTreeSet<String> = candidates
        .iter()
        .filter(|endpoint| endpoint.trust == Trust::Trusted)
        .map(|endpoint| endpoint.uri.to_string())
        .collect();
    if !trusted.is_empty() {
        return Expected::Trusted(trusted);
    }

    let is_excluded = |endpoint: &Endpoint| excluded_operator.as_ref() == Some(&endpoint.operator);
    let any_excluded = candidates.iter().any(|endpoint| is_excluded(endpoint));
    let mut seen: Vec<&Operator> = Vec::new();
    let mut preferred = Vec::new();
    let mut rest = BTreeSet::new();
    for endpoint in candidates.iter().filter(|endpoint| !is_excluded(endpoint)) {
        if seen.contains(&&endpoint.operator) {
            continue;
        }
        seen.push(&endpoint.operator);
        if endpoint.source == Source::Configured {
            preferred.push(endpoint.uri.to_string());
        } else {
            rest.insert(endpoint.uri.to_string());
        }
    }
    if preferred.is_empty() && rest.is_empty() {
        return Expected::Refused(match excluded_operator {
            Some(operator) if any_excluded => {
                NoEligibleDestinations::AllBelongToSyncOperator(operator)
            }
            _ => NoEligibleDestinations::Empty(case.transport),
        });
    }
    Expected::Untrusted { preferred, rest }
}

fn observed(case: &Case, set: &DestinationServerSet) -> Expected {
    let sync = case.sync.as_ref().map(|(_, sync)| sync.uri.clone());
    match set.draw(case.transport, sync.as_ref(), &Health::default()) {
        Err(refusal) => Expected::Refused(refusal),
        Ok(draw) => {
            let named: Vec<String> = draw
                .destinations()
                .iter()
                .map(ToString::to_string)
                .collect();
            let all_trusted = draw
                .destinations()
                .iter()
                .all(|uri| set.trust_of(uri) == Trust::Trusted);
            if draw.preferred() == 0 && all_trusted {
                Expected::Trusted(named.into_iter().collect())
            } else {
                let (preferred, rest) = named.split_at(draw.preferred());
                Expected::Untrusted {
                    preferred: preferred.to_vec(),
                    rest: rest.iter().cloned().collect(),
                }
            }
        }
    }
}

fn assert_safe(case: &Case, set: &DestinationServerSet) {
    let sync = case.sync.as_ref().map(|(_, sync)| sync);
    let Ok(draw) = set.draw(
        case.transport,
        sync.map(|sync| &sync.uri),
        &Health::default(),
    ) else {
        return;
    };
    let known: Vec<&Endpoint> = case
        .configured
        .iter()
        .map(|(_, configured)| configured)
        .chain(sync)
        .chain(&case.registry)
        .collect();
    let lookup = |uri: &Uri| {
        *known
            .iter()
            .find(|endpoint| endpoint.uri == *uri)
            .unwrap_or_else(|| panic!("the draw named {uri}, which no source holds: {case:?}"))
    };
    let drawn: Vec<&Endpoint> = draw.destinations().iter().map(lookup).collect();

    assert!(
        !drawn.is_empty(),
        "a successful draw names a Destination: {case:?}"
    );
    for endpoint in &drawn {
        assert_ne!(
            endpoint.role,
            Role::Sync,
            "a sync-only indexer received a broadcast: {case:?}"
        );
        if case.transport == Transport::Clearnet {
            assert_ne!(
                endpoint.source,
                Source::Registry,
                "a clearnet draw named a registry entry: {case:?}"
            );
        }
        if case.transport == Transport::Mixnet {
            assert_eq!(
                endpoint.location,
                Location::Remote,
                "a mixnet draw named a local indexer: {case:?}"
            );
            assert!(
                reachable(endpoint, Transport::Mixnet),
                "a mixnet draw named an endpoint off https port 443: {case:?}"
            );
            if let Some(sync) = sync.filter(|sync| sync.trust == Trust::Untrusted) {
                assert!(
                    endpoint.trust == Trust::Trusted || endpoint.operator != sync.operator,
                    "a mixnet draw named the untrusted sync indexer's operator: {case:?}"
                );
            }
        }
    }
    let trusted = drawn
        .iter()
        .filter(|endpoint| endpoint.trust == Trust::Trusted)
        .count();
    assert!(
        trusted == 0 || trusted == drawn.len(),
        "a draw mixed trusted and untrusted Destinations: {case:?}"
    );
    if trusted == 0 {
        let mut operators: Vec<&Operator> =
            drawn.iter().map(|endpoint| &endpoint.operator).collect();
        let total = operators.len();
        operators.sort();
        operators.dedup();
        assert_eq!(
            operators.len(),
            total,
            "an untrusted draw named one operator twice: {case:?}"
        );
    }
}

#[test]
fn every_case_keeps_the_privacy_properties_and_matches_the_rule() {
    let cases = cases();
    assert!(
        cases.len() > 50_000,
        "the enumeration must cover the whole space, got {}",
        cases.len()
    );
    for case in &cases {
        let set = build(case);
        assert_safe(case, &set);
        assert_eq!(observed(case, &set), oracle(case), "{case:?}");
    }
}

#[test]
fn the_enumeration_reaches_every_outcome() {
    let mut trusted = false;
    let mut preferred = false;
    let mut untrusted_rest = false;
    let mut excluded_refusal = false;
    let mut empty_clearnet = false;
    let mut empty_mixnet = false;
    for case in cases() {
        match oracle(&case) {
            Expected::Trusted(_) => trusted = true,
            Expected::Untrusted {
                preferred: head,
                rest,
            } => {
                preferred |= !head.is_empty();
                untrusted_rest |= !rest.is_empty();
            }
            Expected::Refused(NoEligibleDestinations::AllBelongToSyncOperator(_)) => {
                excluded_refusal = true;
            }
            Expected::Refused(NoEligibleDestinations::Empty(Transport::Clearnet)) => {
                empty_clearnet = true;
            }
            Expected::Refused(NoEligibleDestinations::Empty(Transport::Mixnet)) => {
                empty_mixnet = true;
            }
        }
    }
    assert!(trusted && preferred && untrusted_rest);
    assert!(excluded_refusal && empty_clearnet && empty_mixnet);
}

#[test]
fn configured_broadcast_indexers_lead_in_the_order_given() {
    let first = remote_uri("one", RELAY_OPERATOR, MIXNET_PORT);
    let same_operator = remote_uri("two", RELAY_OPERATOR, MIXNET_PORT);
    let second = remote_uri("three", OTHER_OPERATOR, MIXNET_PORT);
    let sync = remote_uri("node", SYNC_OPERATOR, MIXNET_PORT);
    let broadcast = |uri: &Uri| IndexerConfig::new(uri.clone()).role(Role::Broadcast);
    let mut set = DestinationServerSet::from_uris(Trust::Untrusted, [])
        .with_indexer(broadcast(&first))
        .with_indexer(broadcast(&same_operator))
        .with_indexer(broadcast(&second));
    let draw = set
        .draw(Transport::Clearnet, Some(&sync), &Health::default())
        .expect("three candidates");
    assert_eq!(
        draw.destinations(),
        &[first.clone(), second.clone(), sync.clone()]
    );
    assert_eq!(draw.preferred(), 2);

    set.add_indexer(broadcast(&first).trust(Trust::Untrusted));
    let redrawn = set
        .draw(Transport::Clearnet, Some(&sync), &Health::default())
        .expect("three candidates");
    assert_eq!(
        redrawn.destinations()[0],
        first,
        "a replacement keeps its position"
    );
}

#[test]
fn an_untrusted_endpoint_never_hides_a_trusted_one_on_its_operator() {
    let sync = remote_uri("node", SYNC_OPERATOR, MIXNET_PORT);
    let relay = remote_uri("relay", SYNC_OPERATOR, MIXNET_PORT);
    let set = DestinationServerSet::from_uris(Trust::Untrusted, [])
        .with_indexer(IndexerConfig::new(sync.clone()).trust(Trust::Trusted))
        .with_indexer(IndexerConfig::new(relay).role(Role::Broadcast));
    for transport in [Transport::Clearnet, Transport::Mixnet] {
        let draw = set
            .draw(transport, Some(&sync), &Health::default())
            .expect("the trusted sync indexer");
        assert_eq!(
            draw.destinations(),
            std::slice::from_ref(&sync),
            "{transport:?}"
        );
    }
}

#[test]
fn health_thins_only_the_random_tail() {
    use crate::destination::health::FaultDomain;

    let failing = |health: &mut Health, uri: &Uri| {
        for _ in 0..crate::destination::health::UNHEALTHY_FAILURE_THRESHOLD {
            health.note(
                &crate::destination::Host::of_uri(uri),
                true,
                Some(FaultDomain::Destination),
            );
        }
    };
    let registry: Vec<Uri> = ["a", "b", "c", "d", "e", "f"]
        .iter()
        .map(|label| remote_uri(label, &format!("{label}{OTHER_OPERATOR}"), MIXNET_PORT))
        .collect();
    let relay = remote_uri("relay", RELAY_OPERATOR, MIXNET_PORT);
    let set = DestinationServerSet::from_uris(Trust::Untrusted, registry.clone())
        .with_indexer(IndexerConfig::new(relay.clone()).role(Role::Broadcast));
    let mut health = Health::default();
    failing(&mut health, &relay);
    failing(&mut health, &registry[0]);

    let draw = set
        .draw(Transport::Mixnet, None, &health)
        .expect("candidates remain");
    assert_eq!(
        draw.destinations()[0],
        relay,
        "a preferred indexer is never thinned"
    );
    assert!(!draw.destinations().contains(&registry[0]));
    assert_eq!(draw.destinations().len(), registry.len());

    let trusted = DestinationServerSet::from_uris(Trust::Trusted, registry.clone());
    let mut health = Health::default();
    for uri in &registry {
        failing(&mut health, uri);
    }
    let draw = trusted
        .draw(Transport::Mixnet, None, &health)
        .expect("trusted candidates");
    assert_eq!(
        draw.destinations(),
        registry.as_slice(),
        "trusted Destinations are never thinned"
    );
}

#[test]
fn relaxed_reach_keeps_the_mixnet_rule() {
    let sync: Uri = "http://127.0.0.1:9067".parse().expect("a static uri");
    let registry: Uri = "http://127.0.0.1:9068".parse().expect("a static uri");
    let set = DestinationServerSet::registry_for_tests(
        Trust::Untrusted,
        [(registry.clone(), OTHER_OPERATOR)],
    )
    .with_indexer(IndexerConfig::new(sync.clone()).location(Location::Remote));
    let draw = set
        .draw_reaching(
            Transport::Mixnet,
            Transport::Clearnet,
            Some(&sync),
            &Health::default(),
        )
        .expect("the registry mock");
    assert_eq!(draw.destinations(), &[registry]);
    assert_eq!(
        set.draw(Transport::Mixnet, Some(&sync), &Health::default()),
        Err(NoEligibleDestinations::Empty(Transport::Mixnet)),
        "without the seam the mixnet reaches no plain-http loopback endpoint"
    );
}

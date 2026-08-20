//! The Correspondable trait and the curated Correspondent list.
//!
//! This list is kept deliberately separate from the sync-server list:
//! Correspondents are chosen for reliable transaction relay, sync servers
//! for low query latency, so tuning one must not reshape the other.
//!
//! # Provenance
//!
//! Populated 2026-07-21 from a three-way discovery sweep: the hosh.zec.rocks
//! tracker (via its 2026-04-18 Internet Archive snapshot. The live site was
//! down), the hardcoded server lists of open-source Zcash wallets (Ywallet,
//! Zashi, zingo-mobile, zingo-pc, Cake, Unstoppable, Nerdbank, zecwallet
//! lineage), and a Zcash community-forum / ZecHub web sweep, yielding 130
//! candidate endpoints. Every candidate was then probed live with a
//! `GetLightdInfo` gRPC call. Exactly 19 answered on mainnet, all lightwalletd
//! instances synced to the same chain tip. Every zaino deployment and the
//! entire `lightwalletd.com` and `zcash-infra.com` fleets were dead.
//!
//! The entries below are those 19 survivors deduplicated to ONE endpoint per
//! operator, because the party Correspondent Rotation defends against is the
//! operator, not the DNS name: a uniform pick over an operator-diverse pool
//! spreads sends across accumulating parties, where a pool listing one
//! operator's many regional endpoints would overweight that operator.
//! Live regional variants folded into their operator's single entry:
//! - `zec.rocks:443` also answers as `na.`, `sa.`, `eu.`, `ap.zec.rocks:443`
//!   (`me.`, `zcashd.`, and `zaino.unsafe.` variants were dead).
//! - `us.zec.stardust.rest:443` also answers as `eu.zec.stardust.rest:443`
//!   (`eu2.` and `jp.` were dead).
//!
//! Distinct-operator status is inferred from domains and confirmed as far as
//! observable (the entries resolve to unrelated IPs), but operator identity
//! is ultimately self-asserted, and a sybil operator running several entries
//! would weaken rotation. Operational vetting of this list (liveness over
//! time, relay honesty, operator diversity) is a tracked follow-up. See
//! `docs/adr/0011-nym-mixnet-transmission.md`.
//!
//! # Port-443 restriction
//!
//! This list carries only Transmission targets reached over the mixnet, so an
//! endpoint the mixnet cannot reach is worse than useless: it wastes
//! escalation rounds on a certain failure. A 2026-07-21 paired clearnet/mixnet
//! probe (the `network probe` diagnostic) found a clean split: every port-443
//! Correspondent answered over the mixnet, while all three port-9067 entries
//! (`lwd.zcashexplorer.app`, `zec.alexxiy.top`, `carover0.xyz`) completed the
//! SOCKS5 tunnel but then failed the TLS handshake with an EOF. The mixnet
//! exit gateways relay the standard 443 but mishandle the non-standard
//! lightwalletd port. Their clearnet TLS works, so this is a mixnet-reachability
//! property, not a dead host. Because the list is mixnet-only, they are
//! excluded. Any future entry MUST be port 443 for the same reason, a rule the
//! `every_entry_is_port_443` test enforces.

#![forbid(unsafe_code)]

use http::Uri;

/// Something that can be corresponded with over the mixnet: the party a
/// Transmission addresses, never the path that carries it.
///
/// ```
/// use zingolib::correspondent::Correspondable;
///
/// let indexer = zingolib::indexers::INDEXERS
///     .iter()
///     .find(|indexer| indexer.uri == "https://na.zec.rocks:443")
///     .unwrap();
/// assert_eq!(Correspondable::address(indexer).scheme_str(), Some("https"));
/// assert_eq!(
///     Correspondable::operator(indexer).as_deref(),
///     Some("zec.rocks")
/// );
/// ```
pub trait Correspondable {
    /// Where a Transmission addresses it.
    fn address(&self) -> Uri;
    /// The accountable operator: the draw key and the Health aggregation key.
    fn operator(&self) -> Option<String>;
}

impl Correspondable for zingo_netutils::indexers::Indexer {
    fn address(&self) -> Uri {
        self.uri
            .parse()
            .expect("the census tests pin every entry parseable")
    }

    fn operator(&self) -> Option<String> {
        Some(zingo_netutils::indexers::Indexer::operator(self))
    }
}

#[cfg(feature = "nym")]
impl Correspondable for zingo_price::PriceSource {
    fn address(&self) -> Uri {
        self.url()
            .parse()
            .expect("every price source URL is pinned parseable")
    }

    fn operator(&self) -> Option<String> {
        Some(self.name().to_string())
    }
}

pub mod health;
#[cfg(feature = "nym")]
pub(crate) mod pool;

/// Curated Correspondents (mainnet): the publicly reachable indexers found
/// by the 2026-07-21 discovery sweep, one endpoint per operator, restricted to
/// those the mixnet can actually reach. See the module docs for provenance,
/// the operator-diversity rationale, and the port-443 restriction.
pub const CORRESPONDENT_INDEXERS: &[&str] = &[
    "https://zec.rocks:443",
    "https://us.zec.stardust.rest:443",
    "https://zec-node.cakewallet.com:443",
    "https://lightwalletd.mainnet.cipherscan.app:443",
    "https://lwd.z0n.jp:443",
    "https://l.ombie.cash:443",
    "https://zec.0xrpc.io:443",
    "https://myzec.cryptover.site:443",
    "https://zcashlw.devshore.ovh:443",
    "https://znode.roamerx.win:443",
    "https://webhighway.website:443",
];

/// Parses [`CORRESPONDENT_INDEXERS`] into `Uri`s, skipping any that fail to parse.
///
/// This is the raw curated list, for diagnostic surfaces that carry no wallet
/// data (the `network probe` pairing). A transmission draw MUST NOT use it
/// directly: it goes through `eligible_correspondents` (crate-private, so no
/// intra-doc link from this public item), which enforces the
/// correspondent-is-never-the-sync-indexer invariant (ADR 0022).
pub fn correspondent_indexers() -> Vec<Uri> {
    CORRESPONDENT_INDEXERS
        .iter()
        .filter_map(|entry| entry.parse().ok())
        .collect()
}

/// Nothing safe to draw for a transmission, so the send refuses rather
/// than transmit to the sync indexer.
#[cfg(feature = "nym")]
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NoEligibleCorrespondents {
    /// Every pool entry belongs to the sync indexer's operator.
    #[error(
        "no eligible Correspondent: every entry in the pool belongs to the \
         sync indexer's operator ({0}), and a Correspondent is never \
         allowed to be the sync indexer"
    )]
    AllBelongToSyncOperator(Operator),
    /// The pool held no Correspondents before any exclusion applied.
    #[error("no eligible Correspondent: the pool is empty")]
    EmptyPool,
}

/// The pool a transmission draw is allowed to use: the curated
/// Correspondents minus every entry operated by the party that runs
/// `sync_indexer`.
///
/// This is the sole sanctioned pool constructor for any surface that hands a
/// raw transaction to a drawn indexer (ADR 0022). The sync indexer already
/// holds the wallet's address set, so a draw that lands on it would hand that
/// same party the transmission too, defeating Correspondent Rotation exactly
/// where it matters most. Exclusion is by operator, not by exact URI, for the same
/// reason the list holds one endpoint per operator: `eu.zec.rocks` and
/// `zec.rocks` are the same accumulating party.
///
/// Refuses with [`NoEligibleCorrespondents`] if the exclusion empties the pool,
/// so a misconfigured list fails closed instead of silently transmitting to
/// the sync indexer.
///
/// An Indexerless session passes `None`: it has no accumulating sync
/// operator to exclude, so the ADR 0022 invariant holds vacuously over the
/// full pool (ruling 2026-07-29).
#[cfg(feature = "nym")]
pub(crate) fn eligible_correspondents(
    sync_indexer: Option<&Uri>,
    health: &health::Health,
) -> Result<Vec<Uri>, NoEligibleCorrespondents> {
    let candidates = correspondent_indexers();
    if candidates.is_empty() {
        return Err(NoEligibleCorrespondents::EmptyPool);
    }
    let pool = match sync_indexer {
        Some(sync_indexer) => eligible_from(candidates, sync_indexer)?,
        None => candidates,
    };
    Ok(health.filter_with_floor(pool))
}

/// Pure core of [`eligible_correspondents`], over an arbitrary pool for
/// testability. Crate-visible so the migration draw's pool filtering
/// (`eligible_candidates`) delegates here instead of growing a second,
/// divergent exclusion (ADR 0022 requires one).
#[cfg(feature = "nym")]
pub(crate) fn eligible_from(
    pool: Vec<Uri>,
    sync_indexer: &Uri,
) -> Result<Vec<Uri>, NoEligibleCorrespondents> {
    if pool.is_empty() {
        return Err(NoEligibleCorrespondents::EmptyPool);
    }
    match Operator::of_uri(sync_indexer) {
        None => Ok(pool),
        Some(sync_operator) => {
            let eligible: Vec<Uri> = pool
                .into_iter()
                .filter(|entry| Operator::of_uri(entry).as_ref() != Some(&sync_operator))
                .collect();
            if eligible.is_empty() {
                return Err(NoEligibleCorrespondents::AllBelongToSyncOperator(
                    sync_operator,
                ));
            }
            Ok(eligible)
        }
    }
}

/// Whether two hosts belong to the same accumulating operator: their
/// operator keys match. This is the one predicate every transmission
/// surface uses to compare a candidate against the sync indexer (ADR 0022).
#[cfg(feature = "nym")]
pub(crate) fn same_operator(host_a: &str, host_b: &str) -> bool {
    Operator::of_host(host_a) == Operator::of_host(host_b)
}

/// The accumulating administrative authority behind a Correspondable host, keyed by its registrable parent domain.
#[cfg(feature = "nym")]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Operator(String);

#[cfg(feature = "nym")]
impl Operator {
    /// The Operator of `host`, approximated as the lowercased last two dot-separated labels (the whole host when it has fewer) — an approximation that can only over-exclude, never letting the sync indexer's operator through.
    pub(crate) fn of_host(host: &str) -> Self {
        let host = host.to_ascii_lowercase();
        let labels: Vec<&str> = host.rsplit('.').collect();
        Operator(
            labels
                .iter()
                .take(2)
                .rev()
                .copied()
                .collect::<Vec<_>>()
                .join("."),
        )
    }

    /// The Operator of `uri`'s host, or `None` when the URI has no host.
    pub(crate) fn of_uri(uri: &Uri) -> Option<Self> {
        uri.host().map(Self::of_host)
    }
}

#[cfg(feature = "nym")]
impl std::fmt::Display for Operator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The endpoint-grain identity of a Correspondable host, lowercased because DNS names compare case-insensitively.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Host(String);

impl Host {
    /// The Host a raw host string names, lowercased and otherwise verbatim.
    pub fn of_host_str(candidate: &str) -> Self {
        Host(candidate.to_ascii_lowercase())
    }

    /// The Host of `uri`, falling back to the whole URI's text when it names no host.
    pub fn of_uri(uri: &Uri) -> Self {
        uri.host()
            .map_or_else(|| Host::of_host_str(&uri.to_string()), Host::of_host_str)
    }

    /// The identity as the string the history and the displays render.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<Host> for String {
    fn from(host: Host) -> Self {
        host.0
    }
}

impl From<&Host> for String {
    fn from(host: &Host) -> Self {
        host.0.clone()
    }
}

#[cfg(all(test, feature = "nym"))]
mod tests {
    use super::*;

    #[test]
    fn every_entry_parses() {
        assert_eq!(
            correspondent_indexers().len(),
            CORRESPONDENT_INDEXERS.len(),
            "every Correspondent URI must parse"
        );
    }

    #[test]
    fn every_entry_is_https() {
        // Mixnet transmission is TLS-only end to end, so the exit gateway that
        // terminates the tunnel cannot read or tamper with the traffic. Every
        // Correspondent must be https; the transmit path refuses anything
        // else at dial time.
        for entry in correspondent_indexers() {
            assert_eq!(
                entry.scheme_str(),
                Some("https"),
                "Correspondent entry {entry} must be https"
            );
        }
    }

    #[test]
    fn every_entry_is_port_443() {
        // The mixnet exit gateways relay 443 but mishandle non-standard ports
        // (2026-07-21 paired probe: every port-9067 entry failed the
        // TLS handshake over the tunnel). Since this list is mixnet-only, a
        // non-443 entry is a guaranteed escalation failure. See the module
        // docs.
        for entry in correspondent_indexers() {
            assert_eq!(
                entry.port_u16(),
                Some(443),
                "Correspondent entry {entry} must be port 443 to traverse the mixnet"
            );
        }
    }

    #[test]
    fn one_endpoint_per_operator() {
        // Correspondent Rotation defends against the operator, not the DNS
        // name: no two entries may share a registrable parent domain.
        let operator_domains: Vec<Operator> = CORRESPONDENT_INDEXERS
            .iter()
            .map(|entry| {
                let host = entry
                    .parse::<Uri>()
                    .expect("checked by every_entry_parses")
                    .host()
                    .expect("every Correspondent URI has a host")
                    .to_string();
                Operator::of_host(&host)
            })
            .collect();
        let mut deduped = operator_domains.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(
            deduped.len(),
            operator_domains.len(),
            "two Correspondent entries share an operator domain: {operator_domains:?}"
        );
    }

    #[test]
    fn the_sync_indexers_operator_is_excluded_from_the_correspondent_pool() {
        // A regional variant, not the listed URI, so this fails if exclusion
        // ever weakens to exact-URI matching: the accumulating party is the
        // operator (ADR 0022).
        let sync: Uri = "https://eu.zec.rocks:443".parse().unwrap();
        let pool = eligible_correspondents(Some(&sync), &health::Health::default())
            .expect("ten operators remain");
        assert_eq!(pool.len(), CORRESPONDENT_INDEXERS.len() - 1);
        assert!(
            pool.iter()
                .all(|entry| Operator::of_host(entry.host().unwrap())
                    != Operator::of_host("zec.rocks")),
            "the sync indexer's operator must never appear among the eligible Correspondents"
        );
    }

    #[test]
    fn a_sync_indexer_outside_the_pool_excludes_nothing() {
        let sync: Uri = "https://my.private.indexer.example:443".parse().unwrap();
        let pool = eligible_correspondents(Some(&sync), &health::Health::default())
            .expect("nothing to exclude");
        assert_eq!(pool.len(), CORRESPONDENT_INDEXERS.len());
    }

    #[test]
    fn an_indexerless_session_draws_from_the_full_pool() {
        let pool =
            eligible_correspondents(None, &health::Health::default()).expect("nothing to exclude");
        assert_eq!(pool.len(), CORRESPONDENT_INDEXERS.len());
    }

    /// HYPOTHESIS: a pool owned wholly by the sync indexer's operator
    /// refuses by naming that operator, so the send fails closed instead
    /// of falling back to transmitting through the sync indexer.
    /// Falsified if the refusal is the empty-pool story or renders a
    /// blank operator.
    #[test]
    fn an_emptied_pool_refuses_rather_than_drawing_the_sync_indexer() {
        let sync: Uri = "https://na.zec.rocks:443".parse().unwrap();
        let pool = vec!["https://zec.rocks:443".parse().unwrap()];
        let err = eligible_from(pool, &sync).expect_err("the pool must empty");
        assert_eq!(
            err,
            NoEligibleCorrespondents::AllBelongToSyncOperator(Operator::of_host("zec.rocks"))
        );
        assert!(err.to_string().contains("zec.rocks"), "{err}");
    }

    /// HYPOTHESIS: an empty pool refuses as empty — never as an exclusion
    /// story — whether or not the sync URI resolves an operator.
    /// Falsified if either shape yields an operator-owned refusal or an
    /// operator name in the rendering.
    #[test]
    fn an_empty_pool_refuses_as_empty_never_as_operator_owned() {
        let hostless: Uri = "/no-host".parse().unwrap();
        let resolvable: Uri = "https://na.zec.rocks:443".parse().unwrap();
        for sync in [hostless, resolvable] {
            let err = eligible_from(Vec::new(), &sync).expect_err("an empty pool must refuse");
            assert_eq!(err, NoEligibleCorrespondents::EmptyPool);
            assert!(err.to_string().contains("the pool is empty"), "{err}");
            assert!(!err.to_string().contains("operator"), "{err}");
        }
    }
}

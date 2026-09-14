//! The Destination vocabulary: [`Correspondable`], [`Host`], and [`Operator`].

#![forbid(unsafe_code)]

use http::Uri;

/// Something that can be corresponded with over the mixnet: the party a
/// Transmission addresses, never the path that carries it.
///
/// ```
/// use zingolib::destination::Correspondable;
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
            .expect("the registry tests pin every entry parseable")
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
pub mod rotation;
pub mod servers;

/// Whether two hosts belong to the same accumulating operator: their
/// operator keys match. This is the one predicate every transmission
/// surface uses to compare a candidate against the sync indexer (ADR 0022).
pub(crate) fn same_operator(host_a: &str, host_b: &str) -> bool {
    Operator::of_host(host_a) == Operator::of_host(host_b)
}

/// The accumulating administrative authority behind a Destination host, keyed by its registrable parent domain.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Operator(String);

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

impl std::fmt::Display for Operator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The endpoint-grain identity of a Destination host, lowercased because DNS names compare case-insensitively.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_operator_is_the_registrable_parent_domain_case_insensitively() {
        assert_eq!(
            Operator::of_host("EU.zec.rocks"),
            Operator::of_host("zec.rocks")
        );
        assert_ne!(
            Operator::of_host("zec.rocks"),
            Operator::of_host("zec.rockz")
        );
        assert_eq!(Operator::of_host("localhost").to_string(), "localhost");
    }

    #[test]
    fn same_operator_compares_by_operator_key() {
        assert!(same_operator("eu.zec.rocks", "na.zec.rocks"));
        assert!(!same_operator("eu.zec.rocks", "l.ombie.cash"));
    }
}

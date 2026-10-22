//! The Destination vocabulary: [`Correspondable`] and [`Host`].

#![forbid(unsafe_code)]

use http::Uri;

pub trait Correspondable {
    /// Where a Transmission addresses it.
    fn address(&self) -> Uri;
    /// The accountable operator, where one is known.
    fn operator(&self) -> Option<String>;
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

//! The indexer census: the sole source of truth for lightwalletd/indexer
//! endpoints across zingolib, zingo-cli, and zingo-mobile.
//!
//! Before this module, five places each held their own copy of "which
//! indexers exist": zingolib's config defaults, the attach gate's health
//! target, the spawned `nym-proxy` binary's health target, zingo-cli's
//! uptime leaderboard snapshot, and zingo-mobile's static server list. The
//! copies had already drifted (the mobile testnet default carried `:443`
//! while the config parser completed the portless config default to
//! `:9067`). Every consumer now consults this census; none holds a literal.
//!
//! Update policy mirrors ADR 0021's webpki bundle: the census must never
//! age silently. Entries change on ecosystem cadence, and the
//! `hosh.zec.rocks` monitor is the observation source for uptime-derived
//! entries — refresh [`MOST_UP_INDEXER_URIS`] from its leaderboard and
//! stamp the date below when doing so.
//!
//! Membership rules, pinned by the census tests at the bottom:
//! - every URI is `https` with an explicit port (no completion rules);
//! - no duplicate URIs.
//!
//! Mixnet-side selection (webpki-chained certs only, 443 preferred,
//! operator disjointness from the sync indexer) is policy and lives with
//! the wallet's gates, which read this data.

/// The chain an indexer serves. Deliberately minimal — the census needs to
/// partition entries, not model consensus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexerChain {
    /// Zcash mainnet.
    Main,
    /// Zcash testnet.
    Test,
}

/// One census entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Indexer {
    /// The full `https` URI with an explicit port.
    pub uri: &'static str,
    /// The chain this indexer serves.
    pub chain: IndexerChain,
    /// The translation-key suffix a frontend renders the region with
    /// (zingo-mobile's `settings.<key>`); empty when unknown.
    pub region_key: &'static str,
    /// Kept for recognition and history, never offered for selection.
    pub obsolete: bool,
}

impl Indexer {
    /// The operating party, derived as the endpoint host's registrable
    /// domain (its last two dot-separated labels): one domain is one
    /// administrative authority, so endpoints group by this value for
    /// operator-disjointness decisions.
    ///
    /// ```
    /// use zingo_netutils::indexers::INDEXERS;
    ///
    /// let na = INDEXERS
    ///     .iter()
    ///     .find(|indexer| indexer.uri == "https://na.zec.rocks:443")
    ///     .unwrap();
    /// assert_eq!(na.operator(), "zec.rocks");
    ///
    /// let lwd1 = INDEXERS
    ///     .iter()
    ///     .find(|indexer| indexer.uri == "https://lwd1.zcash-infra.com:9067")
    ///     .unwrap();
    /// assert_eq!(lwd1.operator(), "zcash-infra.com");
    /// ```
    pub fn operator(&self) -> String {
        let uri: http::Uri = self
            .uri
            .parse()
            .expect("the census tests pin every entry parseable");
        let host = uri
            .host()
            .expect("the census tests pin an authority on every entry");
        match host.rmatch_indices('.').nth(1) {
            Some((second_to_last_dot, _)) => host[second_to_last_dot + 1..].to_string(),
            None => host.to_string(),
        }
    }
}

/// The census, chains interleaved; use [`active`] for the common partition.
pub const INDEXERS: &[Indexer] = &[
    Indexer {
        uri: "https://zec.rocks:443",
        chain: IndexerChain::Main,
        region_key: "usa",
        obsolete: false,
    },
    Indexer {
        uri: "https://na.zec.rocks:443",
        chain: IndexerChain::Main,
        region_key: "na",
        obsolete: false,
    },
    Indexer {
        uri: "https://sa.zec.rocks:443",
        chain: IndexerChain::Main,
        region_key: "sa",
        obsolete: false,
    },
    Indexer {
        uri: "https://eu.zec.rocks:443",
        chain: IndexerChain::Main,
        region_key: "ea",
        obsolete: false,
    },
    Indexer {
        uri: "https://ap.zec.rocks:443",
        chain: IndexerChain::Main,
        region_key: "ao",
        obsolete: false,
    },
    Indexer {
        uri: "https://testnet.zec.rocks:443",
        chain: IndexerChain::Test,
        region_key: "",
        obsolete: false,
    },
    // The retired fleet, kept for recognition (a wallet configured years
    // ago still names these).
    Indexer {
        uri: "https://lwd1.zcash-infra.com:9067",
        chain: IndexerChain::Main,
        region_key: "usa",
        obsolete: true,
    },
    Indexer {
        uri: "https://lwd2.zcash-infra.com:9067",
        chain: IndexerChain::Main,
        region_key: "hk",
        obsolete: true,
    },
    Indexer {
        uri: "https://lwd3.zcash-infra.com:9067",
        chain: IndexerChain::Main,
        region_key: "usa",
        obsolete: true,
    },
    Indexer {
        uri: "https://lwd4.zcash-infra.com:9067",
        chain: IndexerChain::Main,
        region_key: "canada",
        obsolete: true,
    },
    Indexer {
        uri: "https://lwd5.zcash-infra.com:9067",
        chain: IndexerChain::Main,
        region_key: "france",
        obsolete: true,
    },
    Indexer {
        uri: "https://lwd6.zcash-infra.com:9067",
        chain: IndexerChain::Main,
        region_key: "usa",
        obsolete: true,
    },
    Indexer {
        uri: "https://lwd7.zcash-infra.com:9067",
        chain: IndexerChain::Main,
        region_key: "netherlands",
        obsolete: true,
    },
    Indexer {
        uri: "https://lwd8.zcash-infra.com:9067",
        chain: IndexerChain::Main,
        region_key: "uk",
        obsolete: true,
    },
    Indexer {
        uri: "https://mainnet.lightwalletd.com:9067",
        chain: IndexerChain::Main,
        region_key: "na",
        obsolete: true,
    },
    Indexer {
        uri: "https://na.lightwalletd.com:443",
        chain: IndexerChain::Main,
        region_key: "na",
        obsolete: true,
    },
    Indexer {
        uri: "https://sa.lightwalletd.com:443",
        chain: IndexerChain::Main,
        region_key: "sa",
        obsolete: true,
    },
    Indexer {
        uri: "https://eu.lightwalletd.com:443",
        chain: IndexerChain::Main,
        region_key: "ea",
        obsolete: true,
    },
    Indexer {
        uri: "https://ai.lightwalletd.com:443",
        chain: IndexerChain::Main,
        region_key: "ao",
        obsolete: true,
    },
    Indexer {
        uri: "https://zaino.unsafe.zec.rocks:443",
        chain: IndexerChain::Main,
        region_key: "",
        obsolete: false,
    },
    Indexer {
        uri: "https://zaino.testnet.unsafe.zec.rocks:443",
        chain: IndexerChain::Test,
        region_key: "",
        obsolete: false,
    },
    Indexer {
        uri: "https://zec-node.cakewallet.com:443",
        chain: IndexerChain::Main,
        region_key: "",
        obsolete: false,
    },
    Indexer {
        uri: "https://lwd.zcashexplorer.app:9067",
        chain: IndexerChain::Main,
        region_key: "",
        obsolete: false,
    },
    Indexer {
        uri: "https://us.zec.stardust.rest:443",
        chain: IndexerChain::Main,
        region_key: "usa",
        obsolete: false,
    },
    Indexer {
        uri: "https://eu.zec.stardust.rest:443",
        chain: IndexerChain::Main,
        region_key: "ea",
        obsolete: false,
    },
    Indexer {
        uri: "https://l.ombie.cash:443",
        chain: IndexerChain::Main,
        region_key: "",
        obsolete: false,
    },
    Indexer {
        uri: "https://z.ombie.cash:443",
        chain: IndexerChain::Main,
        region_key: "",
        obsolete: false,
    },
    Indexer {
        uri: "https://zec.0xrpc.io:443",
        chain: IndexerChain::Main,
        region_key: "",
        obsolete: false,
    },
    Indexer {
        uri: "https://zec.alexxiy.top:9067",
        chain: IndexerChain::Main,
        region_key: "",
        obsolete: false,
    },
    Indexer {
        uri: "https://zec.alexxiy.top:8137",
        chain: IndexerChain::Main,
        region_key: "",
        obsolete: false,
    },
    Indexer {
        uri: "https://lwd.z0n.jp:443",
        chain: IndexerChain::Main,
        region_key: "",
        obsolete: false,
    },
    Indexer {
        uri: "https://lwd.blakyniica.xyz:443",
        chain: IndexerChain::Main,
        region_key: "",
        obsolete: false,
    },
    Indexer {
        uri: "https://carover0.xyz:9067",
        chain: IndexerChain::Main,
        region_key: "",
        obsolete: false,
    },
    Indexer {
        uri: "https://myzec.cryptover.site:443",
        chain: IndexerChain::Main,
        region_key: "",
        obsolete: false,
    },
    Indexer {
        uri: "https://zcashlw.devshore.ovh:443",
        chain: IndexerChain::Main,
        region_key: "",
        obsolete: false,
    },
    Indexer {
        uri: "https://znode.roamerx.win:443",
        chain: IndexerChain::Main,
        region_key: "",
        obsolete: false,
    },
];

/// Indexer URIs with 100% uptime over 30 days, as reported by the
/// `hosh.zec.rocks` leaderboard. A snapshot, not a liveness claim.
///
/// Source: <https://hosh.zec.rocks/zec/leaderboard>
/// Last updated: 2026-03-26
pub const MOST_UP_INDEXER_URIS: &[&str] = &[
    "https://zecnode.sarl:443",
    "https://zwallet.techly.fyi:443",
    "https://zw.run.place:443",
    "https://light.tracier.space:443",
    "https://lightapi.justneedto.click:9067",
    "https://webhighway.website:443",
    "https://zcash.johndo.men:443",
    "https://zecwal.sandycat.cc:443",
    "https://lw.chponks.site:443",
    "https://z.miscthings.casa:9067",
    "https://sn-hub.de:9067",
    "https://zec.rollrunner.info:443",
];

/// The chain's selectable (non-obsolete) entries, census order.
pub fn active(chain: IndexerChain) -> impl Iterator<Item = &'static Indexer> {
    INDEXERS
        .iter()
        .filter(move |indexer| indexer.chain == chain && !indexer.obsolete)
}

/// The chain's entries reachable over the mixnet: selectable members on
/// port 443, the only port the exit policy carries (ADR 0029).
///
/// ```
/// use zingo_netutils::indexers::{IndexerChain, mixnet_eligible};
///
/// let uris: Vec<&str> = mixnet_eligible(IndexerChain::Main)
///     .map(|indexer| indexer.uri)
///     .collect();
/// assert!(uris.contains(&"https://zec.rocks:443"));
/// assert!(uris.iter().all(|uri| uri.ends_with(":443")));
/// ```
pub fn mixnet_eligible(chain: IndexerChain) -> impl Iterator<Item = &'static Indexer> {
    active(chain).filter(|indexer| {
        indexer
            .uri
            .parse::<http::Uri>()
            .ok()
            .and_then(|uri| uri.port_u16())
            == Some(443)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parses_with_explicit_port(uri: &str) -> bool {
        uri.parse::<http::Uri>()
            .map(|parsed| parsed.scheme_str() == Some("https") && parsed.port_u16().is_some())
            .unwrap_or(false)
    }

    /// HYPOTHESIS (the census membership rules): every entry is an https
    /// URI with an explicit port — no consumer may need a port-completion
    /// rule, the drift the old config default demonstrated.
    #[test]
    fn every_entry_is_https_with_an_explicit_port() {
        for indexer in INDEXERS {
            assert!(
                parses_with_explicit_port(indexer.uri),
                "census entry fails the membership rule: {}",
                indexer.uri
            );
        }
        for uri in MOST_UP_INDEXER_URIS {
            assert!(
                parses_with_explicit_port(uri),
                "leaderboard entry fails the membership rule: {uri}"
            );
        }
    }

    /// HYPOTHESIS: no duplicate URIs anywhere in the census.
    #[test]
    fn no_duplicate_uris() {
        let mut seen = std::collections::HashSet::new();
        for indexer in INDEXERS {
            assert!(seen.insert(indexer.uri), "duplicate: {}", indexer.uri);
        }
    }
}

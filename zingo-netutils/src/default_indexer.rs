#![forbid(unsafe_code)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexerChain {
    Main,
    Test,
}

pub const MAINNET_DEFAULT_INDEXER: &str = "https://mainnet.zainod.zingolabs.dev:443";

/// ```
/// use zingo_netutils::default_indexer::{IndexerChain, MAINNET_DEFAULT_INDEXER, default_indexer};
///
/// let mainnet = default_indexer(IndexerChain::Main).expect("mainnet has a Default Indexer");
/// assert_eq!(mainnet.to_string(), format!("{MAINNET_DEFAULT_INDEXER}/"));
/// assert_eq!(mainnet.port_u16(), Some(443));
/// assert!(default_indexer(IndexerChain::Test).is_none());
/// ```
pub fn default_indexer(chain: IndexerChain) -> Option<http::Uri> {
    match chain {
        IndexerChain::Main => Some(
            MAINNET_DEFAULT_INDEXER
                .parse()
                .expect("MAINNET_DEFAULT_INDEXER is a literal the doc-test above pins parseable"),
        ),
        IndexerChain::Test => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mainnet_default_is_https_on_443() {
        let uri = default_indexer(IndexerChain::Main).expect("mainnet has a Default Indexer");
        assert_eq!(uri.scheme_str(), Some("https"));
        assert_eq!(uri.port_u16(), Some(443));
        assert_eq!(uri.host(), Some("mainnet.zainod.zingolabs.dev"));
    }
}

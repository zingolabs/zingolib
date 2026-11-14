#![forbid(unsafe_code)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexerChain {
    Main,
    Test,
}

pub const MAINNET_DEFAULT_INDEXER: &str = "https://mainnet.zainod.zingolabs.dev:443";
pub const TESTNET_DEFAULT_INDEXER: &str = "https://testnet.zec.rocks:443";

/// ```
/// use zingolib_netutils::default_indexer::{
///     IndexerChain, MAINNET_DEFAULT_INDEXER, TESTNET_DEFAULT_INDEXER, default_indexer,
/// };
///
/// let mainnet = default_indexer(IndexerChain::Main);
/// assert_eq!(mainnet.to_string(), format!("{MAINNET_DEFAULT_INDEXER}/"));
/// assert_eq!(mainnet.port_u16(), Some(443));
///
/// let testnet = default_indexer(IndexerChain::Test);
/// assert_eq!(testnet.to_string(), format!("{TESTNET_DEFAULT_INDEXER}/"));
/// assert_eq!(testnet.port_u16(), Some(443));
/// ```
pub fn default_indexer(chain: IndexerChain) -> http::Uri {
    let literal = match chain {
        IndexerChain::Main => MAINNET_DEFAULT_INDEXER,
        IndexerChain::Test => TESTNET_DEFAULT_INDEXER,
    };
    literal
        .parse()
        .expect("both Default Indexer literals are pinned parseable by the doc-test above")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_https_on_443(chain: IndexerChain, host: &str) {
        let uri = default_indexer(chain);
        assert_eq!(uri.scheme_str(), Some("https"));
        assert_eq!(uri.port_u16(), Some(443));
        assert_eq!(uri.host(), Some(host));
    }

    #[test]
    fn the_mainnet_default_is_https_on_443() {
        assert_https_on_443(IndexerChain::Main, "mainnet.zainod.zingolabs.dev");
    }

    #[test]
    fn the_testnet_default_is_https_on_443() {
        assert_https_on_443(IndexerChain::Test, "testnet.zec.rocks");
    }
}

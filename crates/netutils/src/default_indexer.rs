#![forbid(unsafe_code)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexerChain {
    Main,
    Test,
}

pub const MAINNET_DEFAULT_INDEXER: &str = "https://zec.global:443";
pub const TESTNET_DEFAULT_INDEXER: &str = "https://testnet.zec.rocks:443";

/// ```
/// use zl_netutils::default_indexer::{
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
mod tests;

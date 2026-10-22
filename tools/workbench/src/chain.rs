// The chain a measured session syncs: the name zingo-cli's `--chain` takes
// and the indexer every run on that chain pins, so no arm meets a different
// server. Restated here because the workbench is deliberately std-only: the
// spellings mirror `zingolib::config::ChainType::try_from`, the mainnet
// indexer mirrors `zingo_netutils::default_indexer::MAINNET_DEFAULT_INDEXER`,
// and the testnet indexer is the census's `testnet.zec.rocks` until a
// zingolabs testnet host exists. These must not drift from those sources.

/// ```
/// use workbench::chain::Chain;
///
/// assert_eq!(Chain::parse("main"), Ok(Chain::Main));
/// assert_eq!(Chain::parse("mainnet"), Ok(Chain::Main));
/// assert_eq!(Chain::parse("test"), Ok(Chain::Test));
/// assert_eq!(Chain::parse("testnet"), Ok(Chain::Test));
/// assert!(Chain::parse("regtest").is_err());
/// assert_eq!(Chain::default(), Chain::Main);
/// assert_ne!(Chain::Main.indexer(), Chain::Test.indexer());
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Chain {
    #[default]
    Main,
    Test,
}

impl Chain {
    pub fn name(self) -> &'static str {
        match self {
            Chain::Main => "mainnet",
            Chain::Test => "testnet",
        }
    }

    pub fn indexer(self) -> &'static str {
        match self {
            Chain::Main => "https://mainnet.zainod.zingolabs.dev:443",
            Chain::Test => "https://testnet.zec.rocks:443",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "main" | "mainnet" => Ok(Chain::Main),
            "test" | "testnet" => Ok(Chain::Test),
            other => Err(format!("--chain {other}: main or test")),
        }
    }
}

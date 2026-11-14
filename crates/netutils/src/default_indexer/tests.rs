use super::*;

fn assert_https_on_443(chain: IndexerChain, host: &str) {
    let uri = default_indexer(chain);
    assert_eq!(uri.scheme_str(), Some("https"));
    assert_eq!(uri.port_u16(), Some(443));
    assert_eq!(uri.host(), Some(host));
}

#[test]
fn the_mainnet_default_is_https_on_443() {
    assert_https_on_443(IndexerChain::Main, "zec.global");
}

#[test]
fn the_testnet_default_is_https_on_443() {
    assert_https_on_443(IndexerChain::Test, "testnet.zec.rocks");
}

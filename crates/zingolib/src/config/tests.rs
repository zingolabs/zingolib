use crate::config::ChainType;
use crate::config::ClientConfig;

#[tokio::test]
async fn test_load_clientconfig() {
    let valid_uri = crate::config::construct_indexer_uri(
        zl_netutils::default_indexer::MAINNET_DEFAULT_INDEXER.to_string(),
    )
    .expect("MAINNET_DEFAULT_INDEXER is a literal the netutils doc-test pins parseable");

    let temp_dir = tempfile::TempDir::new().unwrap();
    let temp_path = temp_dir.path().to_path_buf();

    let valid_config = ClientConfig::builder()
        .set_indexer_uri(valid_uri.clone())
        .set_chain_type(ChainType::Mainnet)
        .set_wallet_dir(temp_path)
        .build()
        .unwrap();

    assert_eq!(valid_config.indexer_uri(), Some(valid_uri));
    assert_eq!(valid_config.chain_type(), ChainType::Mainnet);
}

/// The Library Birthday must land at or above Sapling activation on the
/// public chains, so a NewSeed wallet built from it starts scanning at
/// the library floor rather than the bottom of the chain.
#[test]
fn lib_birthday_exceeds_sapling_activation() {
    use zcash_protocol::consensus::{NetworkUpgrade, Parameters};

    for chain in [ChainType::Mainnet, ChainType::Testnet] {
        let sapling_activation = chain
            .activation_height(NetworkUpgrade::Sapling)
            .expect("public chains have a sapling activation height");
        assert!(
            crate::config::lib_birthday(chain) > u32::from(sapling_activation),
            "lib_birthday({chain}) must exceed sapling activation"
        );
    }
}

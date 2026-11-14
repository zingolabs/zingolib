use super::ChainType;
use zingo_common_components::protocol::ActivationHeights;

#[test]
fn test_chaintypes_from_strings() {
    assert_eq!(
        ChainType::try_from("mainnet").expect("'mainnet' names the main network"),
        ChainType::Mainnet
    );
    assert_eq!(
        ChainType::try_from("testnet").expect("'testnet' names the test network"),
        ChainType::Testnet
    );
    // the following 2 variants match the string from get_lightd_info, from the LightdInfo struct
    // as referenced in librustzcash/zcash_client_backend/src/proto/service.rs 17a4830c70
    // and provided by an indexer (previously called lightwallet)
    assert_eq!(
        ChainType::try_from("main").expect("'main' names the main network"),
        ChainType::Mainnet
    );
    assert_eq!(
        ChainType::try_from("test").expect("'test' names the test network"),
        ChainType::Testnet
    );
    assert_eq!(
        ChainType::try_from("regtest").expect("'regtest' names the regtest network"),
        ChainType::Regtest(ActivationHeights::default())
    );
    assert!(ChainType::try_from("invalid").is_err());
    let err = ChainType::try_from("badvalue").expect_err("'badvalue' names no network");
    assert_eq!(err.0, "badvalue");
    assert!(ChainType::try_from("").is_err());
    assert!(ChainType::try_from("Mainnet").is_err());
    assert!(ChainType::try_from("MAINNET").is_err());
    assert!(ChainType::try_from("Testnet").is_err());
    assert!(ChainType::try_from("Regtest").is_err());
    assert!(ChainType::try_from("mainnet ").is_err()); // trailing space
    assert!(ChainType::try_from(" mainnet").is_err()); // leading space
    assert!(ChainType::try_from("regtест").is_err()); // lookalike chars
}

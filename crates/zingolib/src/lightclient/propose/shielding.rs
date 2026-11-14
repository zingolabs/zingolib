use zcash_protocol::consensus::Parameters;
use zingo_test_vectors::seeds;

use crate::{
    config::{ClientConfig, WalletConfig},
    lightclient::LightClient,
    testutils::default_test_wallet_settings,
    wallet::error::ProposeShieldError,
};

async fn create_basic_client() -> LightClient {
    let config = ClientConfig::builder()
        .set_wallet_config(WalletConfig::MnemonicPhrase {
            mnemonic_phrase: seeds::HOSPITAL_MUSEUM_SEED.to_string(),
            no_of_accounts: 1.try_into().unwrap(),
            birthday: 419200,
            wallet_settings: default_test_wallet_settings(),
        })
        .build()
        .unwrap();
    LightClient::new(config, true).await.unwrap()
}

#[tokio::test]
async fn propose_shield_missing_scan_prerequisite() {
    let basic_client = create_basic_client().await;
    let propose_shield_result = basic_client
        .wallet()
        .write()
        .await
        .create_shield_proposal(zip32::AccountId::ZERO);
    match propose_shield_result {
        Err(ProposeShieldError::Component(
            zcash_client_backend::data_api::error::Error::ScanRequired,
        )) => true,
        _ => panic!("Unexpected error state!"),
    };
}
#[tokio::test]
async fn get_transparent_addresses() {
    let basic_client = create_basic_client().await;
    let network = basic_client.chain_type();

    // TODO: store t addrs as concrete types instead of encoded
    let transparent_addresses = basic_client
        .wallet()
        .read()
        .await
        .transparent_addresses()
        .values()
        .map(|address| {
            crate::wallet::keys::transparent_address_from_encoded(network.network_type(), address)
        })
        .collect::<Result<Vec<_>, zcash_address::ParseError>>()
        .unwrap();

    assert_eq!(
        transparent_addresses,
        [
            zcash_transparent::address::TransparentAddress::PublicKeyHash([
                161, 138, 222, 242, 254, 121, 71, 105, 93, 131, 177, 31, 59, 185, 120, 148, 255,
                189, 198, 33
            ])
        ]
    );
}

//! all tests below (and in this mod) use example wallets, which describe real-world chains.

use zingo_test_vectors::seeds;

use crate::{
    config::{ClientConfig, WalletConfig},
    lightclient::{LightClient, error::LightClientError},
    mocks::proposal::ProposalBuilder,
    testutils::default_test_wallet_settings,
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
async fn complete_and_transmit_unconnected_error() {
    let mut lc = create_basic_client().await;
    let proposal = ProposalBuilder::default().build();
    let err = lc.send(proposal, zip32::AccountId::ZERO).await.unwrap_err();
    assert!(matches!(err, LightClientError::Offline));
}

use libtonode_tests::chain_generics::RegtestEnvironment;
use zcash_wallet_interface::{BlockHeight, Wallet as _};
use zingo_wallet::ZingoWallet;
use zingolib::testutils::chain_generics::conduct_chain::ConductChain;
use zingolib::testutils::chain_generics::networked::TestnetEnvironment;

async fn connect(single_server_url: String) {
    let mut wallet = ZingoWallet::new_wallet().await;
    wallet
        .add_key(zingo_test_vectors::seeds::ABANDON_ART_SEED.to_owned())
        .await
        .unwrap();
    wallet
        .begin_scanning_server_range(single_server_url, None, None)
        .await
        .unwrap();
}

async fn connect_fixture<CC>()
where
    CC: ConductChain,
{
    let chain = CC::setup().await;
    connect(chain.lightserver_uri().unwrap().to_string()).await;
}

#[tokio::test]
#[test_log::test]
async fn regtest() {
    connect_fixture::<RegtestEnvironment>().await;
}
#[tokio::test]
#[test_log::test]
async fn testnet() {
    connect_fixture::<TestnetEnvironment>().await;
}

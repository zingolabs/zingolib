use libtonode_tests::chain_generics::RegtestEnvironment;
use zcash_wallet_interface::Wallet as _;
use zingo_wallet::ZingoWallet;
use zingolib::testutils::chain_generics::conduct_chain::ConductChain;
use zingolib::testutils::chain_generics::networked::TestnetEnvironment;

async fn connect<CC>()
where
    CC: ConductChain,
{
    let chain = CC::setup().await;
    let mut wallet = ZingoWallet::new_wallet().await;
    wallet
        .add_key(zingo_test_vectors::seeds::ABANDON_ART_SEED.to_owned())
        .await
        .unwrap();
    wallet
        .add_server(chain.lightserver_uri().unwrap().to_string())
        .await
        .unwrap();
}

#[tokio::test]
#[test_log::test]
async fn regtest() {
    connect::<RegtestEnvironment>().await;
}
#[tokio::test]
#[test_log::test]
async fn testnet() {
    connect::<TestnetEnvironment>().await;
}

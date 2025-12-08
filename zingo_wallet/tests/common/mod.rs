//! A mod that will include common stuff to tests. Once they exist.

use zcash_wallet_interface::Wallet as _;
use zingo_wallet::ZingoWallet;

pub(super) async fn connect(single_server_url: &str) {
    let mut wallet = ZingoWallet::new_wallet().await;
    wallet
        .add_key(zingo_test_vectors::seeds::ABANDON_ART_SEED.to_owned())
        .await
        .unwrap();
    wallet
        .add_server(single_server_url.to_owned())
        .await
        .unwrap();
}

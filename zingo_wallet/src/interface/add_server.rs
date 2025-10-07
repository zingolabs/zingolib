use std::num::NonZeroU32;
use std::path::PathBuf;
use std::str::FromStr as _;
use std::sync::Arc;
use std::sync::RwLock;

use http::Uri;

use zingolib::config::ChainType;
use zingolib::config::SyncConfig;
use zingolib::config::ZingoConfig;
use zingolib::lightclient::LightClient;
use zingolib::wallet::LightWallet;
use zingolib::wallet::WalletSettings;
#[derive(thiserror::Error, Debug)]
pub enum AddServerError {
    #[error(
        "Zingo currently can only connect to a lightwallet if it has exactly one key. Try calling add_key."
    )]
    NeedsSingleSeed,
}
pub(crate) async fn add_server(
    zingo_wallet: &mut super::ZingoWallet,
    server_address: String,
) -> Result<(), AddServerError> {
    if zingo_wallet.keys.len() == 1 {
        if let Some(key) = zingo_wallet.keys.get(0) {
            let server_uri = Uri::from_str(server_address.as_str());

            let min_confirmations: NonZeroU32 = NonZeroU32::try_from(1).expect("1 aint 0");
            let sync_config: SyncConfig = 0;
            let wallet_settings: WalletSettings = WalletSettings {
                sync_config,
                min_confirmations,
            }
            let lightwalletd_uri: Arc<RwLock<Uri>> = 0;
            let network_name: ChainType = 0;
            let wallet_dir: Option<PathBuf> = None;
            let wallet_name: PathBuf = 0;
            let logfile_name: PathBuf = 0;
            let wallet_settings: WalletSettings = 0;
            let no_of_accounts: NonZeroU32 = 0;
            let wallet_base;
            let birthday;
            let wallet = LightWallet::new(network, wallet_base, birthday, wallet_settings);
            let config = ZingoConfig {
                lightwalletd_uri,
                chain,
                wallet_dir,
                wallet_name,
                logfile_name,
                wallet_settings,
                no_of_accounts,
            };
            let overwrite = false;
            let lightclient = LightClient::create_from_wallet(wallet, config, overwrite);
        }
    }
    Err(AddServerError::NeedsSingleSeed)
}

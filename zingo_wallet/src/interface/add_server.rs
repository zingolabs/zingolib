use std::num::NonZeroU32;
use std::path::PathBuf;
use std::str::FromStr as _;
use std::sync::Arc;
use std::sync::RwLock;

use http::Uri;

use zingolib::config::ChainType;
use zingolib::config::SyncConfig;
use zingolib::config::TransparentAddressDiscovery;
use zingolib::config::ZingoConfig;
use zingolib::config::ZingoConfigBuilder;
use zingolib::config::chain_from_str;
use zingolib::lightclient::LightClient;
use zingolib::wallet::LightWallet;
use zingolib::wallet::WalletSettings;
#[derive(thiserror::Error, Debug)]
pub enum AddServerError {
    #[error(
        "Zingo currently can only connect to a lightwallet if it has exactly one key. Try calling add_key."
    )]
    NeedsSingleSeed,
    #[error("URI parse from string '{0}' failed with >{1}<.")]
    CantParseUri(String, http::uri::InvalidUri),
    #[error("Wallet creation failed with >{0}<.")]
    CreateWallet(zingolib::wallet::error::WalletError),
    #[error("Seed parse from string '{0}' failed with >{1}<.")]
    ParseSeed(String, bip0039::Error),
}
pub(crate) async fn add_server(
    zingo_wallet: &mut super::ZingoWallet,
    server_address: String,
) -> Result<(), AddServerError> {
    if zingo_wallet.keys.len() == 1 {
        if let Some(key) = zingo_wallet.keys.get(0) {
            let server_uri = Uri::from_str(server_address.as_str())
                .map_err(|invalid_uri| AddServerError::CantParseUri(server_address, invalid_uri))?;

            let lightwalletd_uri: Arc<RwLock<Uri>> = Arc::new(RwLock::new(server_uri));
            let network_name: ChainType = { chain_from_str(chain_name) };
            let birthday;

            let (network_name, birthday) = {
                // we need to ask the indexer for this information

                }

            // this seems like a lot of set up. Do we really need all this right here??
            let no_of_accounts = NonZeroU32::try_from(1).expect("hard-coded integer"); // seems like this should default. Also why are we stringing it in in two places??

            let wallet_base = zingolib::wallet::WalletBase::Mnemonic {
                mnemonic: bip0039::Mnemonic::from_phrase(key)
                    .map_err(|e| AddServerError::ParseSeed(*key, e))?,
                no_of_accounts,
            };

            let wallet_settings = WalletSettings {
                sync_config: SyncConfig {
                    transparent_address_discovery: TransparentAddressDiscovery::minimal(),
                    performance_level: pepper_sync::config::PerformanceLevel::High,
                },
                min_confirmations: NonZeroU32::try_from(1).expect("1 aint 0"),
            }; // maybe this could be defaulted
            let wallet = LightWallet::new(network_name, wallet_base, birthday, wallet_settings)
                .map_err(AddServerError::CreateWallet)?;
            let config = {
                ZingoConfigBuilder::default()
                    .set_lightwalletd_uri(server_uri)
                    .set_wallet_settings(wallet_settings)
                    .set_no_of_accounts(no_of_accounts)
                    .create()
            };
            let overwrite = false;
            let lightclient = LightClient::create_from_wallet(wallet, config, overwrite);
        }
    }
    Err(AddServerError::NeedsSingleSeed)
}

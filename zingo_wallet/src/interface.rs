use http::Uri;
use zingo_common_components::protocol::block_height::block_height_from_u64;

pub struct ZingoWallet {
    keys: Vec<String>, //todo parsing and keyring
    lightclient: Option<zingolib::lightclient::LightClient>,
}

#[derive(thiserror::Error, Debug)]
pub enum AddServerError {
    #[error(
        "Zingo currently can only connect to a lightwallet if it has exactly one key. Try calling add_key."
    )]
    NeedsSingleSeed,
    #[error("URI parse from string '{0}' failed with >{1}<.")]
    CantParseUri(String, http::uri::InvalidUri),
    #[error("Creating network-client connected to '{0}' failed with >{1}<.")]
    CantCreateClient(Uri, zingo_netutils::GetClientError),
    #[error("Server call returned unexpected result: >{0}<.")]
    Callback(#[from] tonic::Status),
    #[error("Server reported unusable chain: >{0}<.")]
    Chain(#[from] zingolib::config::ChainFromStringError),
    #[error("Server reported overflow block height: >{0}<.")]
    BlockHeight(#[from] zingo_common_components::protocol::block_height::BlockHeightFromU64Error),
    #[error("Wallet creation failed with >{0}<.")]
    CreateWallet(#[from] zingolib::wallet::error::WalletError),
    #[error("Seed parse from string '{0}' failed with >{1}<.")]
    ParseSeed(String, bip0039::Error),
}

#[derive(thiserror::Error, Debug)]
pub enum GetMaxScannedHeightError {
    #[error("Todo")]
    NoServer, //TODO
    #[error("Todo")]
    NoHeightFoundForServer, //TODO
    #[error("Todo")]
    WalletError(zingolib::wallet::error::WalletError),
}

impl zcash_wallet_interface::Wallet for ZingoWallet {
    fn user_agent_id() -> zcash_wallet_interface::UserAgentId {
        struct Version(&'static str);

        const VERSION_STR: &str = "0.0.1";
        const VERSION: Version = Version(VERSION_STR);

        zcash_wallet_interface::UserAgentId {
            name: "zingo_wallet".to_string(),
            version: VERSION.0.to_string(),
        }
    }

    async fn new_wallet() -> Self {
        // we cannot instantiate the current version of the lightclient yet
        // without assumptions about keys and servers
        // which would violate principles of the interface
        // so we dont
        ZingoWallet {
            keys: Vec::new(),
            lightclient: None,
        }
    }

    type AddServerError = AddServerError;

    async fn add_server(&mut self, server_address: String) -> Result<(), Self::AddServerError> {
        use std::num::NonZeroU32;
        use std::path::PathBuf;
        use std::str::FromStr as _;
        use std::sync::Arc;
        use std::sync::RwLock;

        use zingolib::config::ChainType;
        use zingolib::config::SyncConfig;
        use zingolib::config::TransparentAddressDiscovery;
        use zingolib::config::ZingoConfig;
        use zingolib::config::ZingoConfigBuilder;
        use zingolib::config::chain_from_str;
        use zingolib::lightclient::LightClient;
        use zingolib::wallet::LightWallet;
        use zingolib::wallet::WalletSettings;
        if self.keys.len() == 1
            && let Some(key) = self.keys.first()
        {
            let server_uri = Uri::from_str(server_address.as_str())
                .map_err(|invalid_uri| AddServerError::CantParseUri(server_address, invalid_uri))?;

            let lightwalletd_uri: Arc<RwLock<Uri>> = Arc::new(RwLock::new(server_uri.clone()));

            let (chain_type, birthday) = {
                // we need to ask the indexer for this information

                let mut client = zingolib::grpc_client::get_zcb_client(server_uri.clone())
                    .await
                    .map_err(|e| AddServerError::CantCreateClient(server_uri.clone(), e))?;

                let lightd_info = client
                    .get_lightd_info(tonic::Request::new(
                        zcash_client_backend::proto::service::Empty {},
                    ))
                    .await?
                    .into_inner();

                let chain_name = &lightd_info.chain_name;
                let chain_type: ChainType = chain_from_str(chain_name)?;

                let birthday = block_height_from_u64(lightd_info.block_height)?;
                (chain_name, birthday)
            };

            // this seems like a lot of set up. Do we really need all this right here??
            let no_of_accounts = NonZeroU32::try_from(1).expect("hard-coded integer"); // seems like this should default. Also why are we stringing it in in two places??

            let wallet_base = zingolib::wallet::WalletBase::Mnemonic {
                mnemonic: bip0039::Mnemonic::from_phrase(key)
                    .map_err(|e| AddServerError::ParseSeed(key.clone(), e))?,
                no_of_accounts,
            };

            let wallet_settings = WalletSettings {
                sync_config: SyncConfig {
                    transparent_address_discovery: TransparentAddressDiscovery::minimal(),
                    performance_level: pepper_sync::config::PerformanceLevel::High,
                },
                min_confirmations: NonZeroU32::try_from(1).expect("1 aint 0"),
            }; // maybe this could be defaulted
            let wallet = LightWallet::new(chain_type, wallet_base, birthday, wallet_settings)
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
        Err(AddServerError::NeedsSingleSeed)
    }

    type AddKeyError = ();

    async fn add_key(&mut self, key_string: String) -> Result<(), Self::AddKeyError> {
        todo!()
    }

    type GetMaxScannedHeightError = GetMaxScannedHeightError;

    async fn get_max_scanned_height_for_server(
        &mut self,
        server: String,
    ) -> Result<zcash_wallet_interface::BlockHeight, Self::GetMaxScannedHeightError> {
        use zcash_client_backend::data_api::WalletRead;

        match &self.lightclient {
            Some(client) => client
                .wallet
                .read()
                .await
                .chain_height()
                .map_err(GetMaxScannedHeightError::WalletError)?
                .map(|h| zcash_wallet_interface::BlockHeight(h.into()))
                .ok_or(GetMaxScannedHeightError::NoHeightFoundForServer),
            None => Err(GetMaxScannedHeightError::NoServer),
        }
    }

    type PayError = ();

    async fn pay(
        &mut self,
        payments: Vec<zcash_wallet_interface::Payment>,
    ) -> Result<(), Self::PayError> {
        todo!()
    }
}

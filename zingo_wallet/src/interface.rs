use http::Uri;
use zcash_wallet_interface::BlockHeight;

use crate::ZingoWallet;

#[derive(thiserror::Error, Debug)]
pub enum BeginScanningServerRangeError {
    #[error(
        "Requested to only scan up to block {0} but this functionality is not yet implemented."
    )]
    MaximumScanRequested(u32),
    #[error(
        "Zingo currently can only connect to a lightwallet if it has exactly one key. Try calling add_key."
    )]
    NeedsSingleSeed,
    #[error("URI parse from string '{0}' failed with >{1}<.")]
    ParseUri(String, http::uri::InvalidUri),
    #[error("Creating network-client connected to '{0}' failed with >{1}<.")]
    CreateNetworkClient(Uri, zingo_netutils::GetClientError),
    #[error("Server call returned unexpected result: >{0}<.")]
    Callback(#[from] tonic::Status),
    #[error("Server reported unusable chain: >{0}<.")]
    Chain(#[from] zingolib::config::ChainFromStringError),
    #[error("Server reported overflow block height: >{0}<.")]
    BlockHeight(#[from] std::num::TryFromIntError),
    #[error("Seed parse from string '{0}' failed with >{1}<.")]
    ParseSeed(String, bip0039::Error),
    #[error("Wallet creation failed with >{0}<.")]
    CreateLightWallet(#[from] zingolib::wallet::error::WalletError),
    #[error("Temporary data dir creation failed with >{0}<.")]
    CreateDataDir(#[from] std::io::Error),
    #[error("Wallet creation failed with >{0}<.")]
    CreateLightClient(#[from] zingolib::lightclient::error::LightClientError),
    #[error("Wallet creation failed with >{0}<.")]
    StartSync(zingolib::lightclient::error::LightClientError),
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

#[derive(thiserror::Error, Debug)]
pub enum AddKeyError {
    #[error("Todo")]
    AlreadyHasKey, //TODO
}

impl zcash_wallet_interface::Wallet for ZingoWallet {
    fn user_agent_id() -> zcash_wallet_interface::UserAgentId {
        const PARADIGM: &str = "zingo_wallet";
        const VERSION: &str = "0.0.1";

        zcash_wallet_interface::UserAgentId {
            paradigm: PARADIGM.to_string(),
            version: VERSION.to_string(),
        }
    }

    async fn new_wallet() -> Self {
        // we cannot instantiate the current version of the `LightClient` yet
        // without assumptions about keys and servers
        // which would violate principles of the interface
        // so we dont
        ZingoWallet {
            keys: Vec::new(),
            lightclient: None,
        }
    }

    type BeginScanningServerRangeError = BeginScanningServerRangeError;

    async fn begin_scanning_server_range(
        &mut self,
        server_address: String,
        minimum_block: Option<BlockHeight>,
        maximum_block: Option<BlockHeight>,
    ) -> Result<(), Self::BeginScanningServerRangeError> {
        use std::num::NonZeroU32;

        use std::str::FromStr as _;

        use zingolib::config::ChainType;
        use zingolib::config::SyncConfig;
        use zingolib::config::TransparentAddressDiscovery;

        use zingolib::config::ZingoConfigBuilder;
        use zingolib::config::chain_from_str;
        use zingolib::lightclient::LightClient;
        use zingolib::wallet::LightWallet;
        use zingolib::wallet::WalletSettings;

        if let Some(maximum_scan_block) = maximum_block {
            return Err(BeginScanningServerRangeError::MaximumScanRequested(
                maximum_scan_block.0,
            ));
        }

        if self.keys.len() == 1
            && let Some(key) = self.keys.first()
        {
            let server_uri = Uri::from_str(server_address.as_str()).map_err(|invalid_uri| {
                BeginScanningServerRangeError::ParseUri(server_address, invalid_uri)
            })?;

            let (chain_type, birthday) = {
                // we need to ask the indexer for this information

                let mut client = {
                    // global configuration must be manually set *somewhere*
                    rustls::crypto::ring::default_provider().install_default();
                    zingolib::grpc_client::get_zcb_client(server_uri.clone())
                        .await
                        .map_err(|e| {
                            BeginScanningServerRangeError::CreateNetworkClient(
                                server_uri.clone(),
                                e,
                            )
                        })?
                };

                let lightd_info = client
                    .get_lightd_info(tonic::Request::new(
                        zcash_client_backend::proto::service::Empty {},
                    ))
                    .await?
                    .into_inner();

                let chain_name = &lightd_info.chain_name;
                let chain_type: ChainType = chain_from_str(chain_name)?;

                let birthday: zcash_primitives::consensus::BlockHeight = match minimum_block {
                    Some(minimum_block_height) => minimum_block_height.0.into(),
                    None => lightd_info.block_height.try_into()?,
                };
                (chain_type, birthday)
            };

            // this seems like a lot of set up. Do we really need all this right here??
            let no_of_accounts = NonZeroU32::try_from(1).expect("hard-coded integer"); // seems like this should default. Also why are we stringing it in in two places??

            let wallet_base = zingolib::wallet::WalletBase::Mnemonic {
                mnemonic: bip0039::Mnemonic::from_phrase(key)
                    .map_err(|e| BeginScanningServerRangeError::ParseSeed(key.clone(), e))?,
                no_of_accounts,
            };

            let wallet_settings = WalletSettings {
                sync_config: SyncConfig {
                    transparent_address_discovery: TransparentAddressDiscovery::minimal(),
                    performance_level: pepper_sync::config::PerformanceLevel::High,
                },
                min_confirmations: NonZeroU32::try_from(1).expect("1 aint 0"),
            }; // maybe this could be defaulted
            let wallet =
                LightWallet::new(chain_type, wallet_base, birthday, wallet_settings.clone())
                    .map_err(BeginScanningServerRangeError::CreateLightWallet)?;
            // ZingoConfig allows a save-director of None, but crashes if that value is used.
            let save_dir = tempfile::TempDir::new()?;
            let config = {
                ZingoConfigBuilder::default()
                    .set_lightwalletd_uri(server_uri)
                    .set_wallet_settings(wallet_settings)
                    .set_no_of_accounts(no_of_accounts)
                    .set_wallet_dir(save_dir.path().to_path_buf())
                    .create()
            };
            let overwrite = false;
            let mut lightclient = LightClient::create_from_wallet(wallet, config, overwrite)?;
            lightclient
                .sync()
                .await
                .map_err(BeginScanningServerRangeError::StartSync)?;
            self.lightclient = Some(lightclient);
            Ok(())
        } else {
            Err(BeginScanningServerRangeError::NeedsSingleSeed)
        }
    }

    type AddKeyError = AddKeyError;

    async fn add_key(&mut self, key_string: String) -> Result<(), Self::AddKeyError> {
        if self.keys.is_empty() {
            self.keys.push(key_string);
            Ok(())
        } else {
            Err(AddKeyError::AlreadyHasKey)
        }
    }

    type GetMaxScannedHeightError = GetMaxScannedHeightError;

    async fn get_max_scanned_height_for_server(
        &mut self,
        _server: String,
    ) -> Result<zcash_wallet_interface::BlockHeight, Self::GetMaxScannedHeightError> {
        

        match &self.lightclient {
            Some(client) => Ok(client
                .wallet
                .read()
                .await
                .sync_state
                .highest_scanned_height()
                .map(|h| zcash_wallet_interface::BlockHeight(h.into()))
                .unwrap_or(BlockHeight(0))),
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

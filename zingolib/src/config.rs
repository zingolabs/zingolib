//! Module for configuration and construction of a [`crate::lightclient::LightClient`] and/or [`crate::wallet::LightWallet`].

use std::{
    io,
    net::ToSocketAddrs,
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use http::uri::InvalidUri;
use log::info;

use zingo_common_components::protocol::ActivationHeights;

use crate::wallet::WalletSettings;

/// Default indexer uri
pub const DEFAULT_INDEXER_URI: &str = "https://zec.rocks:443";
/// Default indexer uri (testnet)
pub const DEFAULT_INDEXER_URI_TESTNET: &str = "https://testnet.zec.rocks";
/// Default wallet file name
pub const DEFAULT_WALLET_NAME: &str = "zingo-wallet.dat";

/// The network types a lightclient can connect to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainType {
    /// Mainnet
    Mainnet,
    /// Testnet
    Testnet,
    /// Regtest
    Regtest(ActivationHeights),
}

impl std::fmt::Display for ChainType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let chain = match self {
            ChainType::Mainnet => "mainnet",
            ChainType::Testnet => "testnet",
            ChainType::Regtest(_) => "regtest",
        };
        write!(f, "{chain}")
    }
}

impl TryFrom<&str> for ChainType {
    type Error = InvalidChainType;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "mainnet" => Ok(ChainType::Mainnet),
            "testnet" => Ok(ChainType::Testnet),
            "regtest" => Ok(ChainType::Regtest(ActivationHeights::default())),
            _ => Err(InvalidChainType(value.to_string())),
        }
    }
}

pub(crate) mod consealed {
    use zcash_protocol::consensus::{
        BlockHeight, MAIN_NETWORK, NetworkType, NetworkUpgrade, Parameters, TEST_NETWORK,
    };

    use super::ChainType;

    impl Parameters for ChainType {
        fn network_type(&self) -> NetworkType {
            match self {
                ChainType::Mainnet => NetworkType::Main,
                ChainType::Testnet => NetworkType::Test,
                ChainType::Regtest(_) => NetworkType::Regtest,
            }
        }

        fn activation_height(&self, nu: NetworkUpgrade) -> Option<BlockHeight> {
            match self {
                ChainType::Mainnet => MAIN_NETWORK.activation_height(nu),
                ChainType::Testnet => TEST_NETWORK.activation_height(nu),
                ChainType::Regtest(activation_heights) => match nu {
                    NetworkUpgrade::Overwinter => {
                        activation_heights.overwinter().map(BlockHeight::from_u32)
                    }
                    NetworkUpgrade::Sapling => {
                        activation_heights.sapling().map(BlockHeight::from_u32)
                    }
                    NetworkUpgrade::Blossom => {
                        activation_heights.blossom().map(BlockHeight::from_u32)
                    }
                    NetworkUpgrade::Heartwood => {
                        activation_heights.heartwood().map(BlockHeight::from_u32)
                    }
                    NetworkUpgrade::Canopy => {
                        activation_heights.canopy().map(BlockHeight::from_u32)
                    }
                    NetworkUpgrade::Nu5 => activation_heights.nu5().map(BlockHeight::from_u32),
                    NetworkUpgrade::Nu6 => activation_heights.nu6().map(BlockHeight::from_u32),
                    NetworkUpgrade::Nu6_1 => activation_heights.nu6_1().map(BlockHeight::from_u32),
                },
            }
        }
    }
}

/// Invalid chain type.
#[derive(thiserror::Error, Debug)]
#[error("Invalid chain type '{0}'. Expected one of: 'mainnet', 'testnet' or 'regtest'.")]
pub struct InvalidChainType(String);

/// Creates a zingo config for lightclient construction.
#[deprecated(note = "replaced by ZingoConfig builder pattern")]
pub fn load_clientconfig(
    indexer_uri: http::Uri,
    data_dir: Option<PathBuf>,
    chain_type: ChainType,
    wallet_settings: WalletSettings,
    no_of_accounts: NonZeroU32,
    wallet_name: String,
) -> std::io::Result<ZingoConfig> {
    check_indexer_uri(&indexer_uri);
    let wallet_name = wallet_name_or_default(Some(wallet_name));
    let wallet_dir = wallet_dir_or_default(data_dir, chain_type);

    let config = ZingoConfig {
        indexer_uri,
        chain_type,
        wallet_dir,
        wallet_name,
        wallet_settings,
        no_of_accounts,
    };

    Ok(config)
}

/// Constructs a http::Uri from a `server` string. If `server` is `None` use the `DEFAULT_INDEXER_URI`.
/// If the provided string is missing the http prefix, a prefix of `http://` will be added.
/// If the provided string is missing a port, a port of `:9067` will be added.
pub fn construct_lightwalletd_uri(server: Option<String>) -> Result<http::Uri, InvalidUri> {
    match server {
        Some(s) => {
            if s.is_empty() {
                return Ok(http::Uri::default());
            } else {
                let mut s = if s.starts_with("http") {
                    s
                } else {
                    "http://".to_string() + &s
                };
                let uri: http::Uri = s.parse()?;
                if uri.port().is_none() {
                    s += ":9067";
                }
                s
            }
        }
        None => DEFAULT_INDEXER_URI.to_string(),
    }
    .parse()
}

/// Configuration data for the construction of a [`crate::lightclient::LightClient`].
// TODO: this config should only be used to create a lightclient, the data should then be moved into fields of
// lightclient or lightwallet if it needs to retained in memory.
#[derive(Clone, Debug)]
pub struct ZingoConfig {
    /// URI of the indexer the lightclient is connected to.
    indexer_uri: http::Uri,
    /// Chain type of the blockchain the lightclient is connected to.
    chain_type: ChainType,
    /// Directory where the wallet file will be created. By default, this will be in ~/.zcash on Linux and %APPDATA%\Zcash on Windows.
    wallet_dir: PathBuf,
    /// Wallet file name. This will be created in the `wallet_dir`.
    wallet_name: String,
    /// Wallet settings.
    wallet_settings: WalletSettings,
    /// Number of accounts.
    no_of_accounts: NonZeroU32,
}

impl ZingoConfig {
    /// Constructs a default builder.
    #[must_use]
    pub fn builder() -> ZingoConfigBuilder {
        ZingoConfigBuilder::default()
    }

    /// Returns indexer URI.
    #[must_use]
    pub fn indexer_uri(&self) -> http::Uri {
        self.indexer_uri.clone()
    }

    /// Returns wallet directory.
    #[must_use]
    pub fn chain_type(&self) -> ChainType {
        self.chain_type
    }

    /// Returns wallet directory.
    #[must_use]
    pub fn wallet_dir(&self) -> PathBuf {
        self.wallet_dir.clone()
    }

    /// Returns wallet file name.
    #[must_use]
    pub fn wallet_name(&self) -> &str {
        &self.wallet_name
    }

    /// Returns wallet settings..
    #[must_use]
    pub fn wallet_settings(&self) -> WalletSettings {
        self.wallet_settings.clone()
    }

    /// Returns number of accounts..
    #[must_use]
    pub fn no_of_accounts(&self) -> NonZeroU32 {
        self.no_of_accounts
    }

    /// Returns the directory that the Zcash proving parameters are located in.
    pub fn get_zcash_params_path(&self) -> io::Result<Box<Path>> {
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            Ok(self.wallet_dir().into_boxed_path())
        }

        //TODO:  This fn is not correct for regtest mode
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            if dirs::home_dir().is_none() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Couldn't determine home directory!",
                ));
            }

            let zcash_params_dir = zcash_proofs::default_params_folder().unwrap();

            Ok(zcash_params_dir.into_boxed_path())
        }
    }

    /// Returns full path to wallet file.
    #[must_use]
    pub fn get_wallet_path(&self) -> Box<Path> {
        let mut wallet_path = self.wallet_dir();
        wallet_path.push(self.wallet_name());

        wallet_path.into_boxed_path()
    }

    /// Creates a backup file of the current wallet file in the wallet directory.
    // TODO: move to lightclient or lightwallet
    pub fn backup_existing_wallet(&self) -> Result<String, String> {
        if !self.get_wallet_path().exists() {
            return Err(format!(
                "Couldn't find existing wallet to backup. Looked in {}",
                self.get_wallet_path().display()
            ));
        }

        let mut backup_file_path = self.wallet_dir();
        backup_file_path.push(format!(
            "zingo-wallet.backup.{}.dat",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("should never fail when comparing with an instant so far in the past")
                .as_secs()
        ));

        let backup_file_str = backup_file_path.to_string_lossy().to_string();
        std::fs::copy(self.get_wallet_path(), backup_file_path).map_err(|e| format!("{e}"))?;

        Ok(backup_file_str)
    }
}

#[cfg(any(test, feature = "testutils"))]
impl ZingoConfig {
    /// create a `ZingoConfig` that helps a `LightClient` connect to a server.
    #[must_use]
    pub fn create_testnet() -> ZingoConfig {
        ZingoConfig::builder()
            .set_chain_type(ChainType::Testnet)
            .set_indexer_uri((DEFAULT_INDEXER_URI_TESTNET).parse::<http::Uri>().unwrap())
            .build()
    }

    /// create a `ZingoConfig` that helps a `LightClient` connect to a server.
    #[must_use]
    pub fn create_mainnet() -> ZingoConfig {
        ZingoConfig::builder()
            .set_chain_type(ChainType::Mainnet)
            .set_indexer_uri((DEFAULT_INDEXER_URI).parse::<http::Uri>().unwrap())
            .build()
    }

    /// create a `ZingoConfig` that signals a `LightClient` not to connect to a server.
    #[must_use]
    pub fn create_unconnected(chain: ChainType, dir: Option<PathBuf>) -> ZingoConfig {
        if let Some(dir) = dir {
            ZingoConfig::builder()
                .set_chain_type(chain)
                .set_wallet_dir(dir)
                .build()
        } else {
            ZingoConfig::builder().set_chain_type(chain).build()
        }
    }
}

/// Builder for [`ZingoConfig`].
#[derive(Clone, Debug)]
pub struct ZingoConfigBuilder {
    indexer_uri: Option<http::Uri>,
    chain_type: ChainType,
    wallet_dir: Option<PathBuf>,
    wallet_name: Option<String>,
    wallet_settings: WalletSettings,
    no_of_accounts: NonZeroU32,
}

impl ZingoConfigBuilder {
    /// Constructs a new builder for [`ZingoConfig`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Set indexer URI.
    /// # Examples
    /// ```
    /// use zingolib::config::ZingoConfig;
    /// use http::Uri;
    /// let config = ZingoConfig::builder().set_indexer_uri(("https://zcash.mysideoftheweb.com:19067").parse::<Uri>().unwrap()).build();
    /// assert_eq!(config.indexer_uri(), "https://zcash.mysideoftheweb.com:19067");
    /// ```
    pub fn set_indexer_uri(mut self, indexer_uri: http::Uri) -> Self {
        self.indexer_uri = Some(indexer_uri);
        self
    }

    /// Set chain type.
    /// # Examples
    /// ```
    /// use zingolib::config::ZingoConfig;
    /// use zingolib::config::ChainType;
    /// let config = ZingoConfig::builder().set_chain_type(ChainType::Testnet).build();
    /// assert_eq!(config.chain_type(), ChainType::Testnet);
    /// ```
    pub fn set_chain_type(mut self, chain_type: ChainType) -> Self {
        self.chain_type = chain_type;
        self
    }

    /// Set wallet directory.
    /// # Examples
    /// ```
    /// use zingolib::config::ZingoConfig;
    /// use tempfile::TempDir;
    /// let dir = tempfile::TempDir::with_prefix("zingo_doc_test").unwrap().path().to_path_buf();
    /// let config = ZingoConfig::builder().set_wallet_dir(dir.clone()).build();
    /// assert_eq!(config.wallet_dir(), dir);
    /// ```
    pub fn set_wallet_dir(mut self, dir: PathBuf) -> Self {
        self.wallet_dir = Some(dir);
        self
    }

    /// Set wallet file name.
    pub fn set_wallet_name(mut self, wallet_name: String) -> Self {
        self.wallet_name = Some(wallet_name);
        self
    }

    /// Set wallet settings.
    pub fn set_wallet_settings(mut self, wallet_settings: WalletSettings) -> Self {
        self.wallet_settings = wallet_settings;
        self
    }

    /// Set number of accounts.
    pub fn set_no_of_accounts(mut self, no_of_accounts: NonZeroU32) -> Self {
        self.no_of_accounts = no_of_accounts;
        self
    }

    /// Build a [`ZingoConfig`] from the builder.
    pub fn build(self) -> ZingoConfig {
        let wallet_dir = wallet_dir_or_default(self.wallet_dir, self.chain_type);
        let wallet_name = wallet_name_or_default(self.wallet_name);
        ZingoConfig {
            indexer_uri: self.indexer_uri.clone().unwrap_or_default(),
            chain_type: self.chain_type,
            wallet_dir,
            wallet_name,
            wallet_settings: self.wallet_settings,
            no_of_accounts: self.no_of_accounts,
        }
    }
}

impl Default for ZingoConfigBuilder {
    fn default() -> Self {
        Self {
            indexer_uri: None,
            wallet_dir: None,
            wallet_name: None,
            chain_type: ChainType::Mainnet,
            wallet_settings: WalletSettings {
                sync_config: pepper_sync::config::SyncConfig {
                    transparent_address_discovery:
                        pepper_sync::config::TransparentAddressDiscovery::minimal(),
                    performance_level: pepper_sync::config::PerformanceLevel::High,
                },
                min_confirmations: NonZeroU32::try_from(3).unwrap(),
            },
            no_of_accounts: NonZeroU32::try_from(1).expect("hard coded non-zero integer"),
        }
    }
}

// TODO: return errors
fn check_indexer_uri(indexer_uri: &http::Uri) {
    if let Some(host) = indexer_uri.host()
        && let Some(port) = indexer_uri.port()
    {
        match format!("{}:{}", host, port,).to_socket_addrs() {
            Ok(_) => {
                info!("Connected to {indexer_uri}");
            }
            Err(e) => {
                info!("Couldn't resolve server: {e}");
            }
        }
    } else {
        info!("Using offline mode");
    }
}

fn wallet_name_or_default(opt_wallet_name: Option<String>) -> String {
    let wallet_name = opt_wallet_name.unwrap_or_else(|| DEFAULT_WALLET_NAME.into());
    if wallet_name.is_empty() {
        DEFAULT_WALLET_NAME.into()
    } else {
        wallet_name
    }
}

fn wallet_dir_or_default(opt_wallet_dir: Option<PathBuf>, chain: ChainType) -> PathBuf {
    let wallet_dir: PathBuf;
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        // TODO: handle errors
        wallet_dir = opt_wallet_dir.unwrap();
    }

    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        wallet_dir = opt_wallet_dir.clone().unwrap_or_else(|| {
            let mut dir = dirs::data_dir().expect("Couldn't determine user's data directory!");

            #[cfg(any(target_os = "macos", target_os = "windows"))]
            {
                dir.push("Zcash");
            }

            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            {
                dir.push(".zcash");
            }

            match chain {
                ChainType::Mainnet => {}
                ChainType::Testnet => dir.push("testnet3"),
                ChainType::Regtest(_) => dir.push("regtest"),
            }

            dir
        });

        // Create directory if it doesn't exist on non-mobile platforms
        match std::fs::create_dir_all(wallet_dir.clone()) {
            Ok(()) => {}
            Err(e) => {
                panic!("Couldn't create zcash directory!\n {e}");
            }
        }
    }

    wallet_dir
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use pepper_sync::config::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery};

    use crate::{
        config::{ChainType, ZingoConfig},
        wallet::WalletSettings,
    };

    #[tokio::test]
    async fn test_load_clientconfig() {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("Ring to work as a default");
        tracing_subscriber::fmt().init();

        let valid_uri = crate::config::construct_lightwalletd_uri(Some(
            crate::config::DEFAULT_INDEXER_URI.to_string(),
        ))
        .unwrap();
        // let invalid_uri = construct_lightwalletd_uri(Some("Invalid URI".to_string()));
        let temp_dir = tempfile::TempDir::new().unwrap();

        let temp_path = temp_dir.path().to_path_buf();
        // let temp_path_invalid = temp_dir.path().to_path_buf();

        let valid_config = ZingoConfig::builder()
            .set_indexer_uri(valid_uri.clone())
            .set_chain_type(ChainType::Mainnet)
            .set_wallet_dir(temp_path)
            .set_wallet_settings(WalletSettings {
                sync_config: SyncConfig {
                    transparent_address_discovery: TransparentAddressDiscovery::minimal(),
                    performance_level: PerformanceLevel::High,
                },
                min_confirmations: NonZeroU32::try_from(1).unwrap(),
            })
            .set_no_of_accounts(NonZeroU32::try_from(1).expect("hard-coded non-zero integer"))
            .set_wallet_name("".to_string())
            .build();

        assert_eq!(valid_config.indexer_uri(), valid_uri);
        assert_eq!(valid_config.chain_type, ChainType::Mainnet);

        // let invalid_config = load_clientconfig_serverless(
        //     invalid_uri.clone(),
        //     Some(temp_path_invalid),
        //     ChainType::Mainnet,
        //     true,
        // );
        // assert_eq!(invalid_config.is_err(), true);
    }
}

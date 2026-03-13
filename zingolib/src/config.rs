//! `ZingConfig`
//! TODO: Add Crate Description Here!

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::{
    io,
    num::NonZeroU32,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use log::info;

use zcash_protocol::consensus::{
    BlockHeight, MAIN_NETWORK, NetworkType, NetworkUpgrade, Parameters, TEST_NETWORK,
};

use zingo_common_components::protocol::ActivationHeights;

use crate::wallet::WalletSettings;

/// Regtest address for donation in test environments
pub const ZENNIES_FOR_ZINGO_REGTEST_ADDRESS: &str = "uregtest14emvr2anyul683p43d0ck55c04r65ld6f0shetcn77z8j7m64hm4ku3wguf60s75f0g3s7r7g89z22f3ff5tsfgr45efj4pe2gyg5krqp5vvl3afu0280zp9ru2379zat5y6nkqkwjxsvpq5900kchcgzaw8v8z3ggt5yymnuj9hymtv3p533fcrk2wnj48g5vg42vle08c2xtanq0e";

/// Gets the appropriate donation address for the given chain type
#[must_use]
pub fn get_donation_address_for_chain(chain: &ChainType) -> &'static str {
    match chain {
        ChainType::Testnet => ZENNIES_FOR_ZINGO_TESTNET_ADDRESS,
        ChainType::Mainnet => ZENNIES_FOR_ZINGO_DONATION_ADDRESS,
        ChainType::Regtest(_) => ZENNIES_FOR_ZINGO_REGTEST_ADDRESS,
    }
}

/// The networks a zingolib client can run against
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChainType {
    /// Testnet
    Testnet,
    /// Mainnet
    Mainnet,
    /// Regtest
    Regtest(ActivationHeights),
}

impl ChainType {
    const TESTNET_STR: &str = "test";
    const MAINNET_STR: &str = "main";
    const REGTEST_STR: &str = "regtest";

    /// Returns the string label for this chain type.
    pub fn as_str(&self) -> &'static str {
        match self {
            ChainType::Testnet => Self::TESTNET_STR,
            ChainType::Mainnet => Self::MAINNET_STR,
            ChainType::Regtest(_) => Self::REGTEST_STR,
        }
    }
}

impl std::fmt::Display for ChainType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ChainType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            ChainType::MAINNET_STR => Ok(ChainType::Mainnet),
            ChainType::TESTNET_STR => Ok(ChainType::Testnet),
            ChainType::REGTEST_STR => Ok(ChainType::Regtest(ActivationHeights::default())),
            other => Err(format!("Unknown chain type: {other}")),
        }
    }
}

impl Parameters for ChainType {
    fn network_type(&self) -> NetworkType {
        match self {
            ChainType::Testnet => NetworkType::Test,
            ChainType::Mainnet => NetworkType::Main,
            ChainType::Regtest(_) => NetworkType::Regtest,
        }
    }

    fn activation_height(&self, nu: NetworkUpgrade) -> Option<BlockHeight> {
        use ChainType::{Mainnet, Regtest, Testnet};
        match self {
            Testnet => TEST_NETWORK.activation_height(nu),
            Mainnet => MAIN_NETWORK.activation_height(nu),
            Regtest(activation_heights) => match nu {
                NetworkUpgrade::Overwinter => {
                    activation_heights.overwinter().map(BlockHeight::from_u32)
                }
                NetworkUpgrade::Sapling => activation_heights.sapling().map(BlockHeight::from_u32),
                NetworkUpgrade::Blossom => activation_heights.blossom().map(BlockHeight::from_u32),
                NetworkUpgrade::Heartwood => {
                    activation_heights.heartwood().map(BlockHeight::from_u32)
                }
                NetworkUpgrade::Canopy => activation_heights.canopy().map(BlockHeight::from_u32),
                NetworkUpgrade::Nu5 => activation_heights.nu5().map(BlockHeight::from_u32),
                NetworkUpgrade::Nu6 => activation_heights.nu6().map(BlockHeight::from_u32),
                NetworkUpgrade::Nu6_1 => activation_heights.nu6_1().map(BlockHeight::from_u32),
            },
        }
    }
}

/// Testnet donation address.
pub const ZENNIES_FOR_ZINGO_TESTNET_ADDRESS: &str = "utest19zd9laj93deq4lkay48xcfyh0tjec786x6yrng38fp6zusgm0c84h3el99fngh8eks4kxv020r2h2njku6pf69anpqmjq5c3suzcjtlyhvpse0aqje09la48xk6a2cnm822s2yhuzfr47pp4dla9rakdk90g0cee070z57d3trqk87wwj4swz6uf6ts6p5z6lep3xyvueuvt7392tww";

/// Mainnet donation address.
pub const ZENNIES_FOR_ZINGO_DONATION_ADDRESS: &str = "u1p32nu0pgev5cr0u6t4ja9lcn29kaw37xch8nyglwvp7grl07f72c46hxvw0u3q58ks43ntg324fmulc2xqf4xl3pv42s232m25vaukp05s6av9z76s3evsstax4u6f5g7tql5yqwuks9t4ef6vdayfmrsymenqtshgxzj59hdydzygesqa7pdpw463hu7afqf4an29m69kfasdwr494";

/// Default donation amount.
pub const ZENNIES_FOR_ZINGO_AMOUNT: u64 = 1_000_000;

/// The lightserver that handles blockchain requests
pub const DEFAULT_INDEXER_URI: &str = "https://zec.rocks:443";

/// Default indexer uri for testnet.
pub const DEFAULT_TESTNET_INDEXER_URI: &str = "https://testnet.zec.rocks";

/// Parses a URI string into `Some(http::Uri)`.
pub fn some_infallible_uri(s: &str) -> Option<http::Uri> {
    Some(s.parse().unwrap())
}

/// The default wallet file name.
pub const DEFAULT_WALLET_NAME: &str = "zingo-wallet.dat";

/// The default log file name
pub const DEFAULT_LOGFILE_NAME: &str = "zingo-wallet.debug.log";

/// Re-export pepper-sync `SyncConfig` for use with `load_clientconfig`
///
pub use pepper_sync::config::{SyncConfig, TransparentAddressDiscovery};

/// Creates a zingo config for lightclient construction.
pub fn load_clientconfig(
    indexer_uri: Option<http::Uri>,
    data_dir: Option<PathBuf>,
    chain: ChainType,
    wallet_settings: WalletSettings,
    no_of_accounts: NonZeroU32,
    wallet_name: String,
) -> std::io::Result<ZingoConfig> {
    use std::net::ToSocketAddrs;

    let wallet_name_config = if wallet_name.is_empty() {
        DEFAULT_WALLET_NAME
    } else {
        &wallet_name
    };

    let config = match indexer_uri.clone() {
        None => {
            info!("Using offline mode");
            ZingoConfig {
                indexer_uri: Arc::new(RwLock::new(None)),
                chain_type: chain,
                wallet_dir: data_dir,
                wallet_name: wallet_name_config.into(),
                wallet_settings,
                no_of_accounts,
            }
        }
        Some(indexer_uri) => {
            if let (Some(host), Some(port)) = (indexer_uri.host(), indexer_uri.port_u16()) {
                match format!("{host}:{port}").to_socket_addrs() {
                    Ok(_) => info!("Configured indexer: {indexer_uri}"),
                    Err(e) => info!("Couldn't resolve server {host}:{port}: {e}"),
                }
            } else {
                info!("Configured indexer URI is missing host or port: {indexer_uri}");
            }

            ZingoConfig {
                indexer_uri: Arc::new(RwLock::new(Some(indexer_uri))),
                chain_type: chain,
                wallet_dir: data_dir,
                wallet_name: wallet_name_config.into(),
                wallet_settings,
                no_of_accounts,
            }
        }
    };

    Ok(config)
}

/// Builder for constructing a [`ZingoConfig`] for a LightClient.
#[derive(Clone, Debug)]
pub struct ZingoConfigBuilder {
    /// Optional indexer endpoint. `None` means offline.
    pub indexer_uri: Option<http::Uri>,

    /// The network type.
    pub chain_type: ChainType,

    /// Whether to watch the mempool.
    pub monitor_mempool: Option<bool>,

    /// The directory where the wallet and logfiles will be created.
    /// By default, this will be in ~/.zcash on Linux and %APPDATA%\Zcash on Windows.
    /// For mac it is in: ~/Library/Application Support/Zcash
    pub wallet_dir: Option<PathBuf>,
    /// The filename of the wallet. This will be created in the `wallet_dir`.
    pub wallet_name: Option<PathBuf>,
    /// The filename of the logfile. This will be created in the `wallet_dir`.
    pub logfile_name: Option<PathBuf>,
    /// Wallet settings.
    pub wallet_settings: WalletSettings,
    /// Number of accounts
    pub no_of_accounts: NonZeroU32,
}

/// Parameters consumed during `LightClient` and `LightWallet` initialization
/// that are not needed after construction.
#[derive(Clone, Debug)]
pub struct LightClientInitParams {
    /// The chain type (network) for this client.
    pub chain_type: ChainType,
    /// Wallet settings.
    pub wallet_settings: WalletSettings,
    /// Number of accounts to initialize.
    pub no_of_accounts: NonZeroU32,
}

/// Configuration data for the creation of a `LightClient`.
impl ZingoConfigBuilder {
    /// Set the URI of the proxy server we download blockchain information from.
    /// # Examples
    /// ```
    /// use zingolib::config::ZingoConfigBuilder;
    /// use http::Uri;
    /// assert_eq!(ZingoConfigBuilder::default().set_lightwalletd_uri(("https://zcash.mysideoftheweb.com:19067").parse::<Uri>().unwrap()).lightwalletd_uri.clone().unwrap(), "https://zcash.mysideoftheweb.com:19067");
    /// ```
    pub fn set_indexer_uri(&mut self, indexer_uri: Option<http::Uri>) -> &mut Self {
        self.indexer_uri = indexer_uri;
        self
    }

    /// Set the chain the consuming client will interact with.
    /// See <https://github.com/bitcoin/bips/blob/master/bip-0087.mediawiki#coin-type>
    /// for chain types.
    /// Note "chain type" is not a formal standard.
    /// # Examples
    /// ```
    /// use zingolib::config::ZingoConfigBuilder;
    /// use zingolib::config::ChainType::Testnet;
    /// assert_eq!(ZingoConfigBuilder::default().set_chain(Testnet).create().chain, Testnet);
    /// ```
    pub fn set_chain(&mut self, chain: ChainType) -> &mut Self {
        self.chain_type = chain;
        self
    }

    /// TODO: Document this pub fn
    pub fn set_wallet_name(&mut self, wallet_name: String) -> &mut Self {
        self.wallet_name = Some(PathBuf::from(wallet_name));
        self
    }

    /// Set the wallet directory where client transaction data will be stored in a wallet.
    /// # Examples
    /// ```
    /// use zingolib::config::ZingoConfigBuilder;
    /// use tempfile::TempDir;
    /// let dir = tempfile::TempDir::with_prefix("zingo_doc_test").unwrap().into_path();
    /// let config = ZingoConfigBuilder::default().set_wallet_dir(dir.clone()).create();
    /// assert_eq!(config.wallet_dir.clone().unwrap(), dir);
    /// ```
    pub fn set_wallet_dir(&mut self, dir: PathBuf) -> &mut Self {
        self.wallet_dir = Some(dir);
        self
    }

    /// Overrides wallet settings.
    pub fn set_wallet_settings(&mut self, wallet_settings: WalletSettings) -> &mut Self {
        self.wallet_settings = wallet_settings;
        self
    }

    /// Sets how many accounts the wallet should use.
    pub fn set_no_of_accounts(&mut self, no_of_accounts: NonZeroU32) -> &mut Self {
        self.no_of_accounts = no_of_accounts;
        self
    }

    /// Builds a [`ZingoConfig`]. If no indexer is set, it's interpreted as offline.
    pub fn create(&self) -> ZingoConfig {
        let lightwalletd_uri = self.indexer_uri.clone().unwrap_or_default();
        ZingoConfig {
            indexer_uri: Arc::new(RwLock::new(Some(lightwalletd_uri))),
            chain_type: self.chain_type,
            wallet_dir: self.wallet_dir.clone(),
            wallet_name: DEFAULT_WALLET_NAME.into(),
            wallet_settings: self.wallet_settings.clone(),
            no_of_accounts: self.no_of_accounts,
        }
    }
}

impl Default for ZingoConfigBuilder {
    fn default() -> Self {
        ZingoConfigBuilder {
            indexer_uri: None,
            monitor_mempool: None,
            wallet_dir: None,
            wallet_name: None,
            logfile_name: None,
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

/// this config should only be used to create a lightclient, the data should then be moved into fields of
/// lightclient or lightwallet if it needs to retained in memory.
#[derive(Clone, Debug)]
pub struct ZingoConfig {
    /// The indexer URI.
    pub indexer_uri: Arc<RwLock<Option<http::Uri>>>,

    /// The chain type.
    pub chain_type: ChainType,
    /// The directory where the wallet and logfiles will be created. By default, this will be in ~/.zcash on Linux and %APPDATA%\Zcash on Windows.
    pub wallet_dir: Option<PathBuf>,
    /// The filename of the wallet. This will be created in the `wallet_dir`.
    wallet_name: String,
    /// Wallet settings.
    wallet_settings: WalletSettings,
    /// Number of accounts.
    no_of_accounts: NonZeroU32,
}

impl ZingoConfig {
    /// Creates a new `ZingoConfigBuilder` for a given chain type.
    #[must_use]
    pub fn build(chain: ChainType) -> ZingoConfigBuilder {
        ZingoConfigBuilder {
            chain_type: chain,
            ..ZingoConfigBuilder::default()
        }
    }

    #[cfg(any(test, feature = "testutils"))]
    /// create a `ZingoConfig` that helps a `LightClient` connect to a server.
    #[must_use]
    pub fn create_testnet() -> ZingoConfig {
        ZingoConfig::build(ChainType::Testnet)
            .set_indexer_uri(some_infallible_uri(DEFAULT_TESTNET_INDEXER_URI))
            .create()
    }

    #[cfg(any(test, feature = "testutils"))]
    /// create a `ZingoConfig` that helps a `LightClient` connect to a server.
    #[must_use]
    pub fn create_mainnet() -> ZingoConfig {
        ZingoConfig::build(ChainType::Mainnet)
            .set_indexer_uri(some_infallible_uri(DEFAULT_INDEXER_URI))
            .create()
    }

    #[cfg(feature = "testutils")]
    /// create a `ZingoConfig` that signals a `LightClient` not to connect to a server.
    #[must_use]
    pub fn create_unconnected(chain: ChainType, dir: Option<PathBuf>) -> ZingoConfig {
        if let Some(dir) = dir {
            ZingoConfig::build(chain).set_wallet_dir(dir).create()
        } else {
            ZingoConfig::build(chain).create()
        }
    }

    /// Returns the chain type (network).
    #[must_use]
    pub fn network_type(&self) -> ChainType {
        self.chain_type
    }

    /// Returns wallet settings.
    #[must_use]
    pub fn wallet_settings(&self) -> WalletSettings {
        self.wallet_settings.clone()
    }

    /// Returns number of accounts.
    #[must_use]
    pub fn no_of_accounts(&self) -> NonZeroU32 {
        self.no_of_accounts
    }

    /// Returns the wallet directory.
    #[must_use]
    pub fn wallet_dir(&self) -> PathBuf {
        self.wallet_dir.clone().unwrap_or_default()
    }

    /// Convenience wrapper
    #[must_use]
    pub fn sapling_activation_height(&self) -> u64 {
        self.chain_type
            .activation_height(NetworkUpgrade::Sapling)
            .unwrap()
            .into()
    }

    /// Returns the directory where the wallet file is stored.
    #[must_use]
    pub fn get_zingo_wallet_dir(&self) -> Box<Path> {
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            PathBuf::from(&self.wallet_dir.as_ref().unwrap()).into_boxed_path()
        }

        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            let mut zcash_data_location;
            // If there's some --data-dir path provided, use it
            if self.wallet_dir.is_some() {
                zcash_data_location = PathBuf::from(&self.wallet_dir.as_ref().unwrap());
            } else {
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                {
                    zcash_data_location =
                        dirs::data_dir().expect("Couldn't determine app data directory!");
                    zcash_data_location.push("Zcash");
                }

                #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                {
                    if dirs::home_dir().is_none() {
                        log::info!("Couldn't determine home dir!");
                    }
                    zcash_data_location =
                        dirs::home_dir().expect("Couldn't determine home directory!");
                    zcash_data_location.push(".zcash");
                }

                match &self.chain_type {
                    ChainType::Testnet => zcash_data_location.push("testnet3"),
                    ChainType::Mainnet => {}
                    ChainType::Regtest(_) => zcash_data_location.push("regtest"),
                }
            }

            // Create directory if it doesn't exist on non-mobile platforms
            match std::fs::create_dir_all(zcash_data_location.clone()) {
                Ok(()) => {}
                Err(e) => {
                    tracing::error!("Couldn't create zcash directory!\n{e}");
                    panic!("Couldn't create zcash directory!");
                }
            }

            zcash_data_location.into_boxed_path()
        }
    }

    /// Returns the directory where the zcash params are stored.
    pub fn get_zcash_params_path(&self) -> io::Result<Box<Path>> {
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            Ok(PathBuf::from(&self.wallet_dir.as_ref().unwrap()).into_boxed_path())
        }

        //TODO:  This fn is not correct for regtest mode
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            if dirs::home_dir().is_none() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Couldn't determine Home Dir",
                ));
            }

            let zcash_params_dir = zcash_proofs::default_params_folder().unwrap();

            Ok(zcash_params_dir.into_boxed_path())
        }
    }

    /// Returns the indexer's URI.
    #[must_use]
    pub fn get_indexer_uri(&self) -> Option<http::Uri> {
        if self.indexer_uri.read().unwrap().is_some() {
            return Some(
                self.indexer_uri
                    .as_ref()
                    .read()
                    .unwrap()
                    .clone()
                    .expect("Couldn't read configured server URI!")
                    .clone(),
            );
        }
        None
    }

    /// Returns the directory where the specified wallet file is stored.
    #[must_use]
    pub fn get_wallet_with_name_pathbuf(&self, wallet_name: String) -> PathBuf {
        let mut wallet_location = self.get_zingo_wallet_dir().into_path_buf();
        // if id is empty, the name is the default name
        if wallet_name.is_empty() {
            wallet_location.push(&self.wallet_name);
        } else {
            wallet_location.push(wallet_name);
        }
        wallet_location
    }

    /// Returns the directory where the specified wallet file is stored.
    /// This variant returns a [`Box<Path>`].
    #[must_use]
    pub fn get_wallet_with_name_path(&self, wallet_name: String) -> Box<Path> {
        self.get_wallet_with_name_pathbuf(wallet_name)
            .into_boxed_path()
    }

    /// Returns whether the specified wallet file exists.
    #[must_use]
    pub fn wallet_with_name_path_exists(&self, wallet_name: String) -> bool {
        self.get_wallet_with_name_path(wallet_name).exists()
    }

    /// Returns the directory where the wallet file is stored, as a [`PathBuf`].
    #[must_use]
    pub fn get_wallet_pathbuf(&self) -> PathBuf {
        let mut wallet_location = self.get_zingo_wallet_dir().into_path_buf();
        wallet_location.push(&self.wallet_name);
        wallet_location
    }

    /// Returns the directory where the wallet file is stored, as a [`Box<Path>`].
    #[must_use]
    pub fn get_wallet_path(&self) -> Box<Path> {
        self.get_wallet_pathbuf().into_boxed_path()
    }

    /// Returns whether the wallet file exists.
    #[must_use]
    pub fn wallet_path_exists(&self) -> bool {
        self.get_wallet_path().exists()
    }

    // TODO: DO NOT RETURN STRINGS!!
    /// Backups the wallet file to a new file in the same directory.
    pub fn backup_existing_wallet(&self) -> Result<String, String> {
        if !self.wallet_path_exists() {
            return Err(format!(
                "Couldn't find existing wallet to backup. Looked in {:?}",
                self.get_wallet_path().to_str()
            ));
        }

        let mut backup_file_path = self.get_zingo_wallet_dir().into_path_buf();
        backup_file_path.push(format!(
            "zingo-wallet.backup.{}.dat",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        ));

        let backup_file_str = backup_file_path.to_string_lossy().to_string();
        std::fs::copy(self.get_wallet_path(), backup_file_path).map_err(|e| format!("{e}"))?;

        Ok(backup_file_str)
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use crate::{
        config::{DEFAULT_INDEXER_URI, some_infallible_uri},
        wallet::WalletSettings,
    };

    #[tokio::test]
    async fn test_load_clientconfig_serverless() {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("Ring to work as a default");
        tracing_subscriber::fmt().init();

        let valid_uri = some_infallible_uri(DEFAULT_INDEXER_URI);
        // let invalid_uri = construct_lightwalletd_uri(Some("Invalid URI".to_string()));
        let temp_dir = tempfile::TempDir::new().unwrap();

        let temp_path = temp_dir.path().to_path_buf();
        // let temp_path_invalid = temp_dir.path().to_path_buf();

        let valid_config = crate::config::load_clientconfig(
            valid_uri.clone(),
            Some(temp_path),
            crate::config::ChainType::Mainnet,
            WalletSettings {
                sync_config: pepper_sync::config::SyncConfig {
                    transparent_address_discovery:
                        pepper_sync::config::TransparentAddressDiscovery::minimal(),
                    performance_level: pepper_sync::config::PerformanceLevel::High,
                },
                min_confirmations: NonZeroU32::try_from(1).unwrap(),
            },
            1.try_into().unwrap(),
            "".to_string(),
        )
        .unwrap();

        assert!(valid_config.get_indexer_uri().is_some());
        assert_eq!(valid_config.get_indexer_uri(), valid_uri);
        assert_eq!(valid_config.chain_type, crate::config::ChainType::Mainnet);
    }
}

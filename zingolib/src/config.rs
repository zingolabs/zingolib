//! `ZingConfig`
//! TODO: Add Crate Description Here!

use std::{
    io,
    net::ToSocketAddrs,
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use log::info;

use zcash_protocol::consensus::{
    BlockHeight, MAIN_NETWORK, NetworkType, NetworkUpgrade, Parameters, TEST_NETWORK,
};

use zingo_common_components::protocol::ActivationHeights;

use crate::wallet::WalletSettings;

/// TODO: Add Doc Comment Here!
pub const DEVELOPER_DONATION_ADDRESS: &str = "u1w47nzy4z5g9zvm4h2s4ztpl8vrdmlclqz5sz02742zs5j3tz232u4safvv9kplg7g06wpk5fx0k0rx3r9gg4qk6nkg4c0ey57l0dyxtatqf8403xat7vyge7mmen7zwjcgvryg22khtg3327s6mqqkxnpwlnrt27kxhwg37qys2kpn2d2jl2zkk44l7j7hq9az82594u3qaescr3c9v";
/// TODO: Add Doc Comment Here!
pub const ZENNIES_FOR_ZINGO_DONATION_ADDRESS: &str = "u1p32nu0pgev5cr0u6t4ja9lcn29kaw37xch8nyglwvp7grl07f72c46hxvw0u3q58ks43ntg324fmulc2xqf4xl3pv42s232m25vaukp05s6av9z76s3evsstax4u6f5g7tql5yqwuks9t4ef6vdayfmrsymenqtshgxzj59hdydzygesqa7pdpw463hu7afqf4an29m69kfasdwr494";
/// TODO: Add Doc Comment Here!
pub const ZENNIES_FOR_ZINGO_TESTNET_ADDRESS: &str = "utest19zd9laj93deq4lkay48xcfyh0tjec786x6yrng38fp6zusgm0c84h3el99fngh8eks4kxv020r2h2njku6pf69anpqmjq5c3suzcjtlyhvpse0aqje09la48xk6a2cnm822s2yhuzfr47pp4dla9rakdk90g0cee070z57d3trqk87wwj4swz6uf6ts6p5z6lep3xyvueuvt7392tww";
/// Regtest address for donation in test environments
pub const ZENNIES_FOR_ZINGO_REGTEST_ADDRESS: &str = "uregtest14emvr2anyul683p43d0ck55c04r65ld6f0shetcn77z8j7m64hm4ku3wguf60s75f0g3s7r7g89z22f3ff5tsfgr45efj4pe2gyg5krqp5vvl3afu0280zp9ru2379zat5y6nkqkwjxsvpq5900kchcgzaw8v8z3ggt5yymnuj9hymtv3p533fcrk2wnj48g5vg42vle08c2xtanq0e";
/// TODO: Add Doc Comment Here!
pub const ZENNIES_FOR_ZINGO_AMOUNT: u64 = 1_000_000;
/// The lightserver that handles blockchain requests
pub const DEFAULT_LIGHTWALLETD_SERVER: &str = "https://zec.rocks:443";
/// Used for testnet
pub const DEFAULT_TESTNET_LIGHTWALLETD_SERVER: &str = "https://testnet.zec.rocks";
/// TODO: Add Doc Comment Here!
pub const DEFAULT_WALLET_NAME: &str = "zingo-wallet.dat";

/// Gets the appropriate donation address for the given chain type
#[must_use]
pub fn get_donation_address_for_chain(chain: &ChainType) -> &'static str {
    match chain {
        ChainType::Mainnet => ZENNIES_FOR_ZINGO_DONATION_ADDRESS,
        ChainType::Testnet => ZENNIES_FOR_ZINGO_TESTNET_ADDRESS,
        ChainType::Regtest(_) => ZENNIES_FOR_ZINGO_REGTEST_ADDRESS,
    }
}

/// The networks a zingolib client can run against
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
            ChainType::Mainnet => "main",
            ChainType::Testnet => "test",
            ChainType::Regtest(_) => "regtest",
        };
        write!(f, "{chain}")
    }
}

// TODO: can we rework the library so we dont have implement Parameters on the public API facing ChainType?
// this trait impl exposes external (zcash_protocol) types to the public API
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

/// Invalid chain type.
#[derive(thiserror::Error, Debug)]
#[error("Invalid chain type '{0}'. Expected one of: 'mainnet', 'testnet' or 'regtest'.")]
pub struct InvalidChainType(String);

/// Creates a zingo config for lightclient construction.
#[deprecated(note = "replaced by ZingoConfig builder pattern")]
pub fn load_clientconfig(
    lightwallet_uri: http::Uri,
    data_dir: Option<PathBuf>,
    chain: ChainType,
    wallet_settings: WalletSettings,
    no_of_accounts: NonZeroU32,
    wallet_name: String,
) -> std::io::Result<ZingoConfig> {
    check_indexer_uri(&lightwallet_uri);
    let wallet_name = wallet_name_or_default(Some(wallet_name));
    let wallet_dir = wallet_dir_or_default(data_dir, chain);

    let config = ZingoConfig {
        indexer_uri: lightwallet_uri,
        network_type: chain,
        wallet_dir,
        wallet_name,
        wallet_settings,
        no_of_accounts,
    };

    Ok(config)
}

/// Constructs a http::Uri from a `server` string. If `server` is `None` use the `DEFAULT_LIGHTWALLETD_SERVER`.
/// If the provided string is missing the http prefix, a prefix of `http://` will be added.
/// If the provided string is missing a port, a port of `:9067` will be added.
// TODO: handle errors
#[must_use]
pub fn construct_lightwalletd_uri(server: Option<String>) -> http::Uri {
    match server {
        Some(s) => {
            if s.is_empty() {
                return http::Uri::default();
            } else {
                let mut s = if s.starts_with("http") {
                    s
                } else {
                    "http://".to_string() + &s
                };
                let uri: http::Uri = s.parse().unwrap();
                if uri.port().is_none() {
                    s += ":9067";
                }
                s
            }
        }
        None => DEFAULT_LIGHTWALLETD_SERVER.to_string(),
    }
    .parse()
    .unwrap()
}

/// Configuration data for the creation of a `LightClient`.
// TODO: this config should only be used to create a lightclient, the data should then be moved into fields of
// lightclient or lightwallet if it needs to retained in memory.
#[derive(Clone, Debug)]
pub struct ZingoConfig {
    /// The URI of the indexer the lightclient is connected to.
    indexer_uri: http::Uri,
    /// The network type of the blockchain the lightclient is connected to.
    // TODO: change for zingo common public API safe type
    network_type: ChainType,
    /// The directory where the wallet will be created. By default, this will be in ~/.zcash on Linux and %APPDATA%\Zcash on Windows.
    wallet_dir: PathBuf,
    /// The filename of the wallet. This will be created in the `wallet_dir`.
    wallet_name: String,
    /// Wallet settings.
    wallet_settings: WalletSettings,
    /// Number of accounts
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
    pub fn network_type(&self) -> ChainType {
        self.network_type
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
        wallet_path.push(&self.wallet_name);

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
                .unwrap()
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
            .set_network_type(ChainType::Testnet)
            .set_indexer_uri(
                (DEFAULT_TESTNET_LIGHTWALLETD_SERVER)
                    .parse::<http::Uri>()
                    .unwrap(),
            )
            .build()
    }

    /// create a `ZingoConfig` that helps a `LightClient` connect to a server.
    #[must_use]
    pub fn create_mainnet() -> ZingoConfig {
        ZingoConfig::builder()
            .set_network_type(ChainType::Mainnet)
            .set_indexer_uri((DEFAULT_LIGHTWALLETD_SERVER).parse::<http::Uri>().unwrap())
            .build()
    }

    /// create a `ZingoConfig` that signals a `LightClient` not to connect to a server.
    #[must_use]
    pub fn create_unconnected(chain: ChainType, dir: Option<PathBuf>) -> ZingoConfig {
        if let Some(dir) = dir {
            ZingoConfig::builder()
                .set_network_type(chain)
                .set_wallet_dir(dir)
                .build()
        } else {
            ZingoConfig::builder().set_network_type(chain).build()
        }
    }
}

/// Builder for [`ZingoConfig`].
#[derive(Clone, Debug)]
pub struct ZingoConfigBuilder {
    indexer_uri: Option<http::Uri>,
    network_type: ChainType,
    wallet_dir: Option<PathBuf>,
    wallet_name: Option<String>,
    wallet_settings: WalletSettings,
    no_of_accounts: NonZeroU32,
}

impl ZingoConfigBuilder {
    /// Constructs a new builder for [`ZingoConfig`].
    pub fn new() -> Self {
        Self {
            indexer_uri: None,
            wallet_dir: None,
            wallet_name: None,
            network_type: ChainType::Mainnet,
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

    /// Set network type.
    /// # Examples
    /// ```
    /// use zingolib::config::ZingoConfig;
    /// use zingolib::config::ChainType;
    /// let config = ZingoConfig::builder().set_network_type(ChainType::Testnet).build();
    /// assert_eq!(config.network_type(), ChainType::Testnet);
    /// ```
    pub fn set_network_type(mut self, network_type: ChainType) -> Self {
        self.network_type = network_type;
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
        let wallet_dir = wallet_dir_or_default(self.wallet_dir, self.network_type);
        let wallet_name = wallet_name_or_default(self.wallet_name);
        ZingoConfig {
            indexer_uri: self.indexer_uri.clone().unwrap_or_default(),
            network_type: self.network_type,
            wallet_dir,
            wallet_name,
            wallet_settings: self.wallet_settings,
            no_of_accounts: self.no_of_accounts,
        }
    }
}

impl Default for ZingoConfigBuilder {
    fn default() -> Self {
        Self::new()
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
            crate::config::DEFAULT_LIGHTWALLETD_SERVER.to_string(),
        ));
        // let invalid_uri = construct_lightwalletd_uri(Some("Invalid URI".to_string()));
        let temp_dir = tempfile::TempDir::new().unwrap();

        let temp_path = temp_dir.path().to_path_buf();
        // let temp_path_invalid = temp_dir.path().to_path_buf();

        let valid_config = ZingoConfig::builder()
            .set_indexer_uri(valid_uri.clone())
            .set_network_type(ChainType::Mainnet)
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
        assert_eq!(valid_config.network_type, ChainType::Mainnet);

        // let invalid_config = load_clientconfig_serverless(
        //     invalid_uri.clone(),
        //     Some(temp_path_invalid),
        //     ChainType::Mainnet,
        //     true,
        // );
        // assert_eq!(invalid_config.is_err(), true);
    }
}

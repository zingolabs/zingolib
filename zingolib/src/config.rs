//! ZingConfig
//! TODO: Add Crate Discription Here!

#![forbid(unsafe_code)]
#![warn(missing_docs)]
use std::{
    io::{self, Error, ErrorKind},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use log::LevelFilter;
use log4rs::{
    append::rolling_file::{
        policy::compound::{
            roll::fixed_window::FixedWindowRoller, trigger::size::SizeTrigger, CompoundPolicy,
        },
        RollingFileAppender,
    },
    config::{Appender, Root},
    encode::pattern::PatternEncoder,
    filter::threshold::ThresholdFilter,
    Config,
};
use zcash_primitives::consensus::{
    BlockHeight, NetworkConstants, NetworkType, NetworkUpgrade, Parameters, MAIN_NETWORK,
    TEST_NETWORK,
};

/// TODO: Add Doc Comment Here!
pub const DEVELOPER_DONATION_ADDRESS: &str = "u1w47nzy4z5g9zvm4h2s4ztpl8vrdmlclqz5sz02742zs5j3tz232u4safvv9kplg7g06wpk5fx0k0rx3r9gg4qk6nkg4c0ey57l0dyxtatqf8403xat7vyge7mmen7zwjcgvryg22khtg3327s6mqqkxnpwlnrt27kxhwg37qys2kpn2d2jl2zkk44l7j7hq9az82594u3qaescr3c9v";
/// TODO: Add Doc Comment Here!
pub const ZENNIES_FOR_ZINGO_REGTEST_ADDRESS: &str = "uregtest14emvr2anyul683p43d0ck55c04r65ld6f0shetcn77z8j7m64hm4ku3wguf60s75f0g3s7r7g89z22f3ff5tsfgr45efj4pe2gyg5krqp5vvl3afu0280zp9ru2379zat5y6nkqkwjxsvpq5900kchcgzaw8v8z3ggt5yymnuj9hymtv3p533fcrk2wnj48g5vg42vle08c2xtanq0e";
/// TODO: Add Doc Comment Here!
pub const ZENNIES_FOR_ZINGO_TESTNET_ADDRESS: &str = "utest19zd9laj93deq4lkay48xcfyh0tjec786x6yrng38fp6zusgm0c84h3el99fngh8eks4kxv020r2h2njku6pf69anpqmjq5c3suzcjtlyhvpse0aqje09la48xk6a2cnm822s2yhuzfr47pp4dla9rakdk90g0cee070z57d3trqk87wwj4swz6uf6ts6p5z6lep3xyvueuvt7392tww";
/// TODO: Add Doc Comment Here!
pub const ZENNIES_FOR_ZINGO_DONATION_ADDRESS: &str = "u1p32nu0pgev5cr0u6t4ja9lcn29kaw37xch8nyglwvp7grl07f72c46hxvw0u3q58ks43ntg324fmulc2xqf4xl3pv42s232m25vaukp05s6av9z76s3evsstax4u6f5g7tql5yqwuks9t4ef6vdayfmrsymenqtshgxzj59hdydzygesqa7pdpw463hu7afqf4an29m69kfasdwr494";
/// TODO: Add Doc Comment Here!
pub const ZENNIES_FOR_ZINGO_AMOUNT: u64 = 1_000_000;
/// TODO: Add Doc Comment Here!
pub const DEFAULT_LIGHTWALLETD_SERVER: &str = "https://zec.rocks:443";
/// TODO: Add Doc Comment Here!
pub const MAX_REORG: usize = 100;
/// TODO: Add Doc Comment Here!
pub const DEFAULT_WALLET_NAME: &str = "zingo-wallet.dat";
/// TODO: Add Doc Comment Here!
pub const DEFAULT_LOGFILE_NAME: &str = "zingo-wallet.debug.log";
/// TODO: Add Doc Comment Here!
pub const REORG_BUFFER_OFFSET: u32 = 0;
/// TODO: Add Doc Comment Here!
pub const BATCH_SIZE: u64 = 100;

/// TODO: Add Doc Comment Here!
#[cfg(any(target_os = "ios", target_os = "android"))]
pub const GAP_RULE_UNUSED_ADDRESSES: usize = 0;

/// TODO: Add Doc Comment Here!
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub const GAP_RULE_UNUSED_ADDRESSES: usize = 5;

/// TODO: Add Doc Comment Here!
pub fn margin_fee() -> u64 {
    zcash_primitives::transaction::fees::zip317::MARGINAL_FEE.into_u64()
}

/// TODO: Add Doc Comment Here!
pub fn load_clientconfig(
    lightwallet_uri: http::Uri,
    data_dir: Option<PathBuf>,
    chain: ChainType,
    monitor_mempool: bool,
) -> std::io::Result<ZingoConfig> {
    use std::net::ToSocketAddrs;
    format!(
        "{}:{}",
        lightwallet_uri.host().unwrap(),
        lightwallet_uri.port().unwrap()
    )
    .to_socket_addrs()?
    .next()
    .ok_or(std::io::Error::new(
        ErrorKind::ConnectionRefused,
        "Couldn't resolve server!",
    ))?;

    // Create a Light Client Config
    let config = ZingoConfig {
        lightwalletd_uri: Arc::new(RwLock::new(lightwallet_uri)),
        chain,
        monitor_mempool,
        reorg_buffer_offset: REORG_BUFFER_OFFSET,
        wallet_dir: data_dir,
        wallet_name: DEFAULT_WALLET_NAME.into(),
        logfile_name: DEFAULT_LOGFILE_NAME.into(),
        accept_server_txids: false,
    };

    Ok(config)
}

/// TODO: Add Doc Comment Here!
pub fn construct_lightwalletd_uri(server: Option<String>) -> http::Uri {
    match server {
        Some(s) => {
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
        None => DEFAULT_LIGHTWALLETD_SERVER.to_string(),
    }
    .parse()
    .unwrap()
}

/// TODO: Add Doc Comment Here!
#[derive(Clone, Debug)]
pub struct ZingoConfigBuilder {
    /// TODO: Add Doc Comment Here!
    pub lightwalletd_uri: Option<http::Uri>,
    /// TODO: Add Doc Comment Here!
    pub chain: ChainType,
    /// TODO: Add Doc Comment Here!
    pub reorg_buffer_offset: Option<u32>,
    /// TODO: Add Doc Comment Here!
    pub monitor_mempool: Option<bool>,
    /// The directory where the wallet and logfiles will be created. By default, this will be in ~/.zcash on Linux and %APPDATA%\Zcash on Windows. For mac it is in: ~/Library/Application Support/Zcash
    pub wallet_dir: Option<PathBuf>,
    /// The filename of the wallet. This will be created in the `wallet_dir`.
    pub wallet_name: Option<PathBuf>,
    /// The filename of the logfile. This will be created in the `wallet_dir`.
    pub logfile_name: Option<PathBuf>,
    /// If this option is enabled, the LightClient will replace outgoing TxId records with the TxId picked by the server. necessary for darkside.
    pub accept_server_txids: bool,
}

/// Configuration data that is necessary? and sufficient? for the creation of a LightClient.
#[derive(Clone, Debug)]
pub struct ZingoConfig {
    /// TODO: Add Doc Comment Here!
    pub lightwalletd_uri: Arc<RwLock<http::Uri>>,
    /// TODO: Add Doc Comment Here!
    pub chain: ChainType,
    /// TODO: Add Doc Comment Here!
    pub reorg_buffer_offset: u32,
    /// TODO: Add Doc Comment Here!
    pub monitor_mempool: bool,
    /// The directory where the wallet and logfiles will be created. By default, this will be in ~/.zcash on Linux and %APPDATA%\Zcash on Windows.
    pub wallet_dir: Option<PathBuf>,
    /// The filename of the wallet. This will be created in the `wallet_dir`.
    pub wallet_name: PathBuf,
    /// The filename of the logfile. This will be created in the `wallet_dir`.
    pub logfile_name: PathBuf,
    /// If this option is enabled, the LightClient will replace outgoing TxId records with the TxId picked by the server. necessary for darkside.
    pub accept_server_txids: bool,
}

impl ZingoConfigBuilder {
    /// Set the URI of the proxy server we download blockchain information from.
    /// # Examples
    /// ```
    /// use zingolib::config::ZingoConfigBuilder;
    /// use http::Uri;
    /// assert_eq!(ZingoConfigBuilder::default().set_lightwalletd_uri(("https://zcash.mysideoftheweb.com:19067").parse::<Uri>().unwrap()).lightwalletd_uri.clone().unwrap(), "https://zcash.mysideoftheweb.com:19067");
    /// ```
    pub fn set_lightwalletd_uri(&mut self, lightwalletd_uri: http::Uri) -> &mut Self {
        self.lightwalletd_uri = Some(lightwalletd_uri);
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
        self.chain = chain;
        self
    }

    /// Set the wallet directory where client transaction data will be stored in a wallet.
    /// # Examples
    /// ```
    /// use zingolib::config::ZingoConfigBuilder;
    /// use tempdir::TempDir;
    /// let dir = TempDir::new("zingo_doc_test").unwrap().into_path();
    /// let config = ZingoConfigBuilder::default().set_wallet_dir(dir.clone()).create();
    /// assert_eq!(config.wallet_dir.clone().unwrap(), dir);
    /// ```
    pub fn set_wallet_dir(&mut self, dir: PathBuf) -> &mut Self {
        self.wallet_dir = Some(dir);
        self
    }

    /// TODO: Add Doc Comment Here!
    pub fn create(&self) -> ZingoConfig {
        let lightwalletd_uri = self.lightwalletd_uri.clone().unwrap_or_default();
        ZingoConfig {
            lightwalletd_uri: Arc::new(RwLock::new(lightwalletd_uri)),
            chain: self.chain,
            monitor_mempool: false,
            reorg_buffer_offset: REORG_BUFFER_OFFSET,
            wallet_dir: self.wallet_dir.clone(),
            wallet_name: DEFAULT_WALLET_NAME.into(),
            logfile_name: DEFAULT_LOGFILE_NAME.into(),
            accept_server_txids: self.accept_server_txids,
        }
    }
}

impl Default for ZingoConfigBuilder {
    fn default() -> Self {
        ZingoConfigBuilder {
            lightwalletd_uri: None,
            monitor_mempool: None,
            reorg_buffer_offset: None,
            wallet_dir: None,
            wallet_name: None,
            logfile_name: None,
            chain: ChainType::Mainnet,
            accept_server_txids: false,
        }
    }
}

impl ZingoConfig {
    /// TODO: Add Doc Comment Here!
    pub fn build(chain: ChainType) -> ZingoConfigBuilder {
        ZingoConfigBuilder {
            chain,
            ..ZingoConfigBuilder::default()
        }
    }

    #[cfg(any(test, feature = "test-elevation"))]
    /// create a ZingoConfig that helps a LightClient connect to a server.
    pub fn create_testnet() -> ZingoConfig {
        ZingoConfig::build(ChainType::Testnet)
            .set_lightwalletd_uri(
                ("https://zcash.mysideoftheweb.com:19067")
                    .parse::<http::Uri>()
                    .unwrap(),
            )
            .create()
    }

    #[cfg(any(test, feature = "test-elevation"))]
    /// create a ZingoConfig that helps a LightClient connect to a server.
    pub fn create_mainnet() -> ZingoConfig {
        ZingoConfig::build(ChainType::Mainnet)
            .set_lightwalletd_uri(("https://zec.rocks:443").parse::<http::Uri>().unwrap())
            .create()
    }

    #[cfg(feature = "test-elevation")]
    /// create a ZingoConfig that signals a LightClient not to connect to a server.
    pub fn create_unconnected(chain: ChainType, dir: Option<PathBuf>) -> ZingoConfig {
        if let Some(dir) = dir {
            ZingoConfig::build(chain).set_wallet_dir(dir).create()
        } else {
            ZingoConfig::build(chain).create()
        }
    }

    /// Convenience wrapper
    pub fn sapling_activation_height(&self) -> u64 {
        self.chain
            .activation_height(NetworkUpgrade::Sapling)
            .unwrap()
            .into()
    }

    /// TODO: Add Doc Comment Here!
    pub fn orchard_activation_height(&self) -> u64 {
        self.chain
            .activation_height(NetworkUpgrade::Nu5)
            .unwrap()
            .into()
    }

    /// TODO: Add Doc Comment Here!
    pub fn set_data_dir(&mut self, dir_str: String) {
        self.wallet_dir = Some(PathBuf::from(dir_str));
    }

    /// Build the Logging config
    pub fn get_log_config(&self) -> io::Result<Config> {
        let window_size = 3; // log0, log1, log2
        let fixed_window_roller = FixedWindowRoller::builder()
            .build("zingo-wallet-log{}", window_size)
            .unwrap();
        let size_limit = 5 * 1024 * 1024; // 5MB as max log file size to roll
        let size_trigger = SizeTrigger::new(size_limit);
        let compound_policy =
            CompoundPolicy::new(Box::new(size_trigger), Box::new(fixed_window_roller));

        Config::builder()
            .appender(
                Appender::builder()
                    .filter(Box::new(ThresholdFilter::new(LevelFilter::Info)))
                    .build(
                        "logfile",
                        Box::new(
                            RollingFileAppender::builder()
                                .encoder(Box::new(PatternEncoder::new("{d} {l}::{m}{n}")))
                                .build(self.get_log_path(), Box::new(compound_policy))?,
                        ),
                    ),
            )
            .build(
                Root::builder()
                    .appender("logfile")
                    .build(LevelFilter::Debug),
            )
            .map_err(|e| Error::new(ErrorKind::Other, format!("{}", e)))
    }

    /// TODO: Add Doc Comment Here!
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

                match &self.chain {
                    ChainType::Testnet => zcash_data_location.push("testnet3"),
                    ChainType::Regtest(_) => zcash_data_location.push("regtest"),
                    ChainType::Mainnet => {}
                };
            }

            // Create directory if it doesn't exist on non-mobile platforms
            match std::fs::create_dir_all(zcash_data_location.clone()) {
                Ok(_) => {}
                Err(e) => {
                    eprintln!("Couldn't create zcash directory!\n{}", e);
                    panic!("Couldn't create zcash directory!");
                }
            };

            zcash_data_location.into_boxed_path()
        }
    }

    /// TODO: Add Doc Comment Here!
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

            let mut zcash_params = self.get_zingo_wallet_dir().into_path_buf();
            zcash_params.push("..");

            #[cfg(any(target_os = "macos", target_os = "windows"))]
            zcash_params.push("ZcashParams");

            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            zcash_params.push(".zcash-params");

            match std::fs::create_dir_all(zcash_params.clone()) {
                Ok(_) => Ok(zcash_params.into_boxed_path()),
                Err(e) => {
                    eprintln!("Couldn't create zcash params directory\n{}", e);
                    Err(e)
                }
            }
        }
    }

    /// TODO: Add Doc Comment Here!
    pub fn get_lightwalletd_uri(&self) -> http::Uri {
        self.lightwalletd_uri
            .read()
            .expect("Couldn't read configured server URI!")
            .clone()
    }

    /// TODO: Add Doc Comment Here!
    pub fn get_wallet_pathbuf(&self) -> PathBuf {
        let mut wallet_location = self.get_zingo_wallet_dir().into_path_buf();
        wallet_location.push(&self.wallet_name);
        wallet_location
    }

    /// TODO: Add Doc Comment Here!
    pub fn get_wallet_path(&self) -> Box<Path> {
        self.get_wallet_pathbuf().into_boxed_path()
    }

    /// TODO: Add Doc Comment Here!
    pub fn wallet_path_exists(&self) -> bool {
        self.get_wallet_path().exists()
    }

    /// TODO: Add Doc Comment Here!
    #[deprecated(note = "this method was renamed 'wallet_path_exists' for clarity")]
    pub fn wallet_exists(&self) -> bool {
        self.wallet_path_exists()
    }

    /// TODO: Add Doc Comment Here!
    pub fn backup_existing_wallet(&self) -> Result<String, String> {
        if !self.wallet_path_exists() {
            return Err(format!(
                "Couldn't find existing wallet to backup. Looked in {:?}",
                self.get_wallet_path().to_str()
            ));
        }
        use std::time::{SystemTime, UNIX_EPOCH};

        let mut backup_file_path = self.get_zingo_wallet_dir().into_path_buf();
        backup_file_path.push(format!(
            "zingo-wallet.backup.{}.dat",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
        ));

        let backup_file_str = backup_file_path.to_string_lossy().to_string();
        std::fs::copy(self.get_wallet_path(), backup_file_path).map_err(|e| format!("{}", e))?;

        Ok(backup_file_str)
    }

    /// TODO: Add Doc Comment Here!
    pub fn get_log_path(&self) -> Box<Path> {
        let mut log_path = self.get_zingo_wallet_dir().into_path_buf();
        log_path.push(&self.logfile_name);
        //println!("LogFile:\n{}", log_path.to_str().unwrap());

        log_path.into_boxed_path()
    }

    /// Coin Types are specified in public registries to disambiguate coin variants
    /// so that HD wallets can manage multiple currencies.
    ///  <https://github.com/satoshilabs/slips/blob/master/slip-0044.md>
    ///  ZEC is registered as 133 (0x80000085) for MainNet and 1 (0x80000001) for TestNet (all coins)
    #[deprecated(since = "0.1.0", note = "obsolete due to `Parameter` trait methods")]
    pub fn get_coin_type(&self) -> u32 {
        self.chain.coin_type()
    }

    /// TODO: Add Doc Comment Here!
    #[deprecated(since = "0.1.0", note = "obsolete due to `Parameter` trait methods")]
    pub fn hrp_sapling_address(&self) -> &str {
        self.chain.hrp_sapling_payment_address()
    }

    /// TODO: Add Doc Comment Here!
    #[deprecated(since = "0.1.0", note = "obsolete due to `Parameter` trait methods")]
    pub fn hrp_sapling_private_key(&self) -> &str {
        self.chain.hrp_sapling_extended_spending_key()
    }

    /// TODO: Add Doc Comment Here!
    #[deprecated(since = "0.1.0", note = "obsolete due to `Parameter` trait methods")]
    pub fn hrp_sapling_viewing_key(&self) -> &str {
        self.chain.hrp_sapling_extended_full_viewing_key()
    }

    /// TODO: Add Doc Comment Here!
    #[deprecated(since = "0.1.0", note = "obsolete due to `Parameter` trait methods")]
    pub fn base58_pubkey_address(&self) -> [u8; 2] {
        self.chain.b58_pubkey_address_prefix()
    }

    /// TODO: Add Doc Comment Here!
    #[deprecated(since = "0.1.0", note = "obsolete due to `Parameter` trait methods")]
    pub fn base58_script_address(&self) -> [u8; 2] {
        self.chain.b58_script_address_prefix()
    }

    /// TODO: Add Doc Comment Here!
    #[deprecated(since = "0.1.0", note = "prefix not known to be used")]
    pub fn base58_secretkey_prefix(&self) -> [u8; 1] {
        match self.chain {
            ChainType::Testnet | ChainType::Regtest(_) => [0xEF],
            ChainType::Mainnet => [0x80],
        }
    }
}

/// TODO: Add Doc Comment Here!
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChainType {
    /// Public testnet
    Testnet,
    /// Local testnet
    Regtest(RegtestNetwork),
    /// Mainnet
    Mainnet,
}

impl ChainType {
    /// TODO: Add Doc Comment Here!
    #[deprecated(since = "0.1.0", note = "prefix not known to be used")]
    pub fn hrp_orchard_spending_key(&self) -> &str {
        match self {
            ChainType::Testnet => "secret-orchard-sk-test",
            ChainType::Regtest(_) => "secret-orchard-sk-regtest",
            ChainType::Mainnet => "secret-orchard-sk-main",
        }
    }

    /// TODO: Add Doc Comment Here!
    #[deprecated(since = "0.1.0", note = "prefix not known to be used")]
    pub fn hrp_unified_full_viewing_key(&self) -> &str {
        match self {
            ChainType::Testnet => "uviewtest",
            ChainType::Regtest(_) => "uviewregtest",
            ChainType::Mainnet => "uview",
        }
    }
}

impl std::fmt::Display for ChainType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use ChainType::*;
        let name = match self {
            Testnet => "test",
            Regtest(_) => "regtest",
            Mainnet => "main",
        };
        write!(f, "{name}")
    }
}

impl Parameters for ChainType {
    fn network_type(&self) -> NetworkType {
        use ChainType::*;
        match self {
            Mainnet => NetworkType::Main,
            Testnet => NetworkType::Test,
            Regtest(_) => NetworkType::Regtest,
        }
    }

    fn activation_height(&self, nu: NetworkUpgrade) -> Option<BlockHeight> {
        use ChainType::*;
        match self {
            Mainnet => MAIN_NETWORK.activation_height(nu),
            Testnet => TEST_NETWORK.activation_height(nu),
            Regtest(regtest_network) => regtest_network.activation_height(nu),
        }
    }
}

/// TODO: Add Doc Comment Here!
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RegtestNetwork {
    activation_heights: ActivationHeights,
}

impl RegtestNetwork {
    /// TODO: Add Doc Comment Here!
    pub fn new(
        overwinter_activation_height: u64,
        sapling_activation_height: u64,
        blossom_activation_height: u64,
        heartwood_activation_height: u64,
        canopy_activation_height: u64,
        orchard_activation_height: u64,
        nu6_activation_height: u64,
    ) -> Self {
        Self {
            activation_heights: ActivationHeights::new(
                overwinter_activation_height,
                sapling_activation_height,
                blossom_activation_height,
                heartwood_activation_height,
                canopy_activation_height,
                orchard_activation_height,
                nu6_activation_height,
            ),
        }
    }

    /// TODO: Add Doc Comment Here!
    pub fn all_upgrades_active() -> Self {
        Self {
            activation_heights: ActivationHeights::new(1, 1, 1, 1, 1, 1, 1),
        }
    }

    /// TODO: Add Doc Comment Here!
    pub fn set_orchard_and_nu6(custom_activation_height: u64) -> Self {
        Self {
            activation_heights: ActivationHeights::new(
                1,
                1,
                1,
                1,
                1,
                custom_activation_height,
                custom_activation_height,
            ),
        }
    }

    /// Network parameters
    pub fn activation_height(&self, nu: NetworkUpgrade) -> Option<BlockHeight> {
        match nu {
            NetworkUpgrade::Overwinter => Some(
                self.activation_heights
                    .get_activation_height(NetworkUpgrade::Overwinter),
            ),
            NetworkUpgrade::Sapling => Some(
                self.activation_heights
                    .get_activation_height(NetworkUpgrade::Sapling),
            ),
            NetworkUpgrade::Blossom => Some(
                self.activation_heights
                    .get_activation_height(NetworkUpgrade::Blossom),
            ),
            NetworkUpgrade::Heartwood => Some(
                self.activation_heights
                    .get_activation_height(NetworkUpgrade::Heartwood),
            ),
            NetworkUpgrade::Canopy => Some(
                self.activation_heights
                    .get_activation_height(NetworkUpgrade::Canopy),
            ),
            NetworkUpgrade::Nu5 => Some(
                self.activation_heights
                    .get_activation_height(NetworkUpgrade::Nu5),
            ),
            NetworkUpgrade::Nu6 => Some(
                self.activation_heights
                    .get_activation_height(NetworkUpgrade::Nu6),
            ),
        }
    }
}

/// TODO: Add Doc Comment Here!
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActivationHeights {
    overwinter: BlockHeight,
    sapling: BlockHeight,
    blossom: BlockHeight,
    heartwood: BlockHeight,
    canopy: BlockHeight,
    orchard: BlockHeight,
    nu6: BlockHeight,
}

impl ActivationHeights {
    /// TODO: Add Doc Comment Here!
    pub fn new(
        overwinter: u64,
        sapling: u64,
        blossom: u64,
        heartwood: u64,
        canopy: u64,
        orchard: u64,
        nu6: u64,
    ) -> Self {
        Self {
            overwinter: BlockHeight::from_u32(overwinter as u32),
            sapling: BlockHeight::from_u32(sapling as u32),
            blossom: BlockHeight::from_u32(blossom as u32),
            heartwood: BlockHeight::from_u32(heartwood as u32),
            canopy: BlockHeight::from_u32(canopy as u32),
            orchard: BlockHeight::from_u32(orchard as u32),
            nu6: BlockHeight::from_u32(nu6 as u32),
        }
    }

    /// TODO: Add Doc Comment Here!
    pub fn get_activation_height(&self, network_upgrade: NetworkUpgrade) -> BlockHeight {
        match network_upgrade {
            NetworkUpgrade::Overwinter => self.overwinter,
            NetworkUpgrade::Sapling => self.sapling,
            NetworkUpgrade::Blossom => self.blossom,
            NetworkUpgrade::Heartwood => self.heartwood,
            NetworkUpgrade::Canopy => self.canopy,
            NetworkUpgrade::Nu5 => self.orchard,
            NetworkUpgrade::Nu6 => self.nu6,
        }
    }
}

#![forbid(unsafe_code)]
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
use zcash_primitives::{
    consensus::{BlockHeight, NetworkUpgrade, Parameters, MAIN_NETWORK, TEST_NETWORK},
    constants,
};

pub const DEFAULT_LIGHTWALLETD_SERVER: &str = "https://mainnet.lightwalletd.com:9067";
pub const MAX_REORG: usize = 100;
pub const DEFAULT_WALLET_NAME: &str = "zingo-wallet.dat";
pub const DEFAULT_LOGFILE_NAME: &str = "zingo-wallet.debug.log";
pub const REORG_BUFFER_OFFSET: u32 = 0;

#[cfg(any(target_os = "ios", target_os = "android"))]
pub const GAP_RULE_UNUSED_ADDRESSES: usize = 0;

#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub const GAP_RULE_UNUSED_ADDRESSES: usize = 5;

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

/// Configuration data that is necessary? and sufficient? for the creation of a LightClient.
#[derive(Clone, Debug)]
pub struct ZingoConfig {
    pub lightwalletd_uri: Arc<RwLock<http::Uri>>,
    pub chain: ChainType,
    pub reorg_buffer_offset: u32,
    pub monitor_mempool: bool,
    /// The directory where the wallet and logfiles will be created. By default, this will be in ~/.zcash on Linux and %APPDATA%\Zcash on Windows.
    pub wallet_dir: Option<PathBuf>,
    /// The filename of the wallet. This will be created in the `wallet_dir`.
    pub wallet_name: PathBuf,
    /// The filename of the logfile. This will be created in the `wallet_dir`.
    pub logfile_name: PathBuf,
    pub regtest_orchard_activation_height: Option<zcash_primitives::consensus::BlockHeight>,
}

impl ZingoConfig {
    // Create an unconnected (to any server) config to test for local wallet etc...
    pub fn create_unconnected(chain: ChainType, dir: Option<PathBuf>) -> ZingoConfig {
        ZingoConfig {
            lightwalletd_uri: Arc::new(RwLock::new(http::Uri::default())),
            chain,
            monitor_mempool: false,
            reorg_buffer_offset: REORG_BUFFER_OFFSET,
            wallet_dir: dir,
            wallet_name: DEFAULT_WALLET_NAME.into(),
            logfile_name: DEFAULT_LOGFILE_NAME.into(),
            regtest_orchard_activation_height: None,
        }
    }

    //Convenience wrapper
    pub fn sapling_activation_height(&self) -> u64 {
        match self.chain {
            ChainType::Regtest => BlockHeight::from_u32(1).into(),
            _ => self
                .chain
                .activation_height(NetworkUpgrade::Sapling)
                .unwrap()
                .into(),
        }
    }

    pub fn orchard_activation_height(&self) -> u64 {
        match self.chain {
            ChainType::Regtest => self
                .regtest_orchard_activation_height
                .unwrap_or(BlockHeight::from_u32(1))
                .into(),
            _ => self
                .chain
                .activation_height(NetworkUpgrade::Nu5)
                .unwrap()
                .into(),
        }
    }
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
                    ChainType::Regtest => zcash_data_location.push("regtest"),
                    ChainType::Mainnet => {}
                    ChainType::FakeMainnet => zcash_data_location.push("fakemainnet"),
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

    pub fn get_lightwalletd_uri(&self) -> http::Uri {
        self.lightwalletd_uri
            .read()
            .expect("Couldn't read configured server URI!")
            .clone()
    }
    pub fn get_wallet_path(&self) -> Box<Path> {
        let mut wallet_location = self.get_zingo_wallet_dir().into_path_buf();
        wallet_location.push(&self.wallet_name);

        wallet_location.into_boxed_path()
    }

    pub fn wallet_exists(&self) -> bool {
        self.get_wallet_path().exists()
    }

    pub fn backup_existing_wallet(&self) -> Result<String, String> {
        if !self.wallet_exists() {
            return Err(format!(
                "Couldn't find existing wallet to backup. Looked in {:?}",
                self.get_wallet_path().to_str()
            ));
        }
        use std::time::{SystemTime, UNIX_EPOCH};

        let mut backup_file_path = self.get_zingo_wallet_dir().into_path_buf();
        backup_file_path.push(&format!(
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

    pub fn get_log_path(&self) -> Box<Path> {
        let mut log_path = self.get_zingo_wallet_dir().into_path_buf();
        log_path.push(&self.logfile_name);
        //println!("LogFile:\n{}", log_path.to_str().unwrap());

        log_path.into_boxed_path()
    }

    pub fn get_coin_type(&self) -> u32 {
        self.chain.coin_type()
    }

    pub fn hrp_sapling_address(&self) -> &str {
        self.chain.hrp_sapling_payment_address()
    }

    pub fn hrp_sapling_private_key(&self) -> &str {
        self.chain.hrp_sapling_extended_spending_key()
    }

    pub fn hrp_sapling_viewing_key(&self) -> &str {
        self.chain.hrp_sapling_extended_full_viewing_key()
    }

    pub fn base58_pubkey_address(&self) -> [u8; 2] {
        self.chain.b58_pubkey_address_prefix()
    }

    pub fn base58_script_address(&self) -> [u8; 2] {
        self.chain.b58_script_address_prefix()
    }

    pub fn base58_secretkey_prefix(&self) -> [u8; 1] {
        match self.chain {
            ChainType::Testnet | ChainType::Regtest | ChainType::FakeMainnet => [0xEF],
            ChainType::Mainnet => [0x80],
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChainType {
    Testnet,
    Regtest,
    Mainnet,
    FakeMainnet,
}

impl ChainType {
    pub fn hrp_orchard_spending_key(&self) -> &str {
        match self {
            ChainType::Testnet => "secret-orchard-sk-test",
            ChainType::Regtest => "secret-orchard-sk-regtest",
            ChainType::Mainnet => "secret-orchard-sk-main",
            ChainType::FakeMainnet => "secret-orchard-sk-main",
        }
    }
    pub fn hrp_unified_full_viewing_key(&self) -> &str {
        match self {
            ChainType::Testnet => "uviewtest",
            ChainType::Regtest => "uviewregtest",
            ChainType::Mainnet => "uview",
            ChainType::FakeMainnet => "uview",
        }
    }
    pub fn to_zcash_address_network(&self) -> zcash_address::Network {
        match self {
            Mainnet | FakeMainnet => zcash_address::Network::Main,
            Testnet => zcash_address::Network::Test,
            Regtest => zcash_address::Network::Regtest,
        }
    }
}

impl std::fmt::Display for ChainType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use ChainType::*;
        let name = match self {
            Testnet => "test",
            Regtest => "regtest",
            Mainnet => "main",
            FakeMainnet => "fakemainnet",
        };
        write!(f, "{name}")
    }
}

use ChainType::*;
impl Parameters for ChainType {
    fn activation_height(&self, nu: NetworkUpgrade) -> Option<BlockHeight> {
        match self {
            Mainnet => MAIN_NETWORK.activation_height(nu),
            Testnet => TEST_NETWORK.activation_height(nu),
            Regtest => None,
            FakeMainnet => Some(BlockHeight::from_u32(1)),
        }
    }

    fn coin_type(&self) -> u32 {
        match self {
            Mainnet | FakeMainnet => constants::mainnet::COIN_TYPE,
            Testnet => constants::testnet::COIN_TYPE,
            Regtest => constants::regtest::COIN_TYPE,
        }
    }

    fn hrp_sapling_extended_spending_key(&self) -> &str {
        match self {
            Mainnet | FakeMainnet => constants::mainnet::HRP_SAPLING_EXTENDED_SPENDING_KEY,
            Testnet => constants::testnet::HRP_SAPLING_EXTENDED_SPENDING_KEY,
            Regtest => constants::regtest::HRP_SAPLING_EXTENDED_SPENDING_KEY,
        }
    }

    fn hrp_sapling_extended_full_viewing_key(&self) -> &str {
        match self {
            Mainnet | FakeMainnet => constants::mainnet::HRP_SAPLING_EXTENDED_FULL_VIEWING_KEY,
            Testnet => constants::testnet::HRP_SAPLING_EXTENDED_FULL_VIEWING_KEY,
            Regtest => constants::regtest::HRP_SAPLING_EXTENDED_FULL_VIEWING_KEY,
        }
    }

    fn hrp_sapling_payment_address(&self) -> &str {
        match self {
            Mainnet | FakeMainnet => constants::mainnet::HRP_SAPLING_PAYMENT_ADDRESS,
            Testnet => constants::testnet::HRP_SAPLING_PAYMENT_ADDRESS,
            Regtest => constants::regtest::HRP_SAPLING_PAYMENT_ADDRESS,
        }
    }

    fn b58_pubkey_address_prefix(&self) -> [u8; 2] {
        match self {
            Mainnet | FakeMainnet => constants::mainnet::B58_PUBKEY_ADDRESS_PREFIX,
            Testnet => constants::testnet::B58_PUBKEY_ADDRESS_PREFIX,
            Regtest => constants::regtest::B58_PUBKEY_ADDRESS_PREFIX,
        }
    }

    fn b58_script_address_prefix(&self) -> [u8; 2] {
        match self {
            Mainnet | FakeMainnet => constants::mainnet::B58_SCRIPT_ADDRESS_PREFIX,
            Testnet => constants::testnet::B58_SCRIPT_ADDRESS_PREFIX,
            Regtest => constants::regtest::B58_SCRIPT_ADDRESS_PREFIX,
        }
    }

    fn address_network(&self) -> Option<zcash_address::Network> {
        Some(self.to_zcash_address_network())
    }
}

use std::{
    io::{self, Error, ErrorKind},
    path::{Path, PathBuf},
};

use log::{info, LevelFilter};
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

pub const DEFAULT_SERVER: &str = "https://zuul.free2z.cash";
pub const MAX_REORG: usize = 100;
pub const WALLET_NAME: &str = "zingo-wallet.dat";
pub const LOGFILE_NAME: &str = "zingo-wallet.debug.log";
pub const ANCHOR_OFFSET: [u32; 5] = [4, 0, 0, 0, 0];
pub const GAP_RULE_UNUSED_ADDRESSES: usize = if cfg!(any(target_os = "ios", target_os = "android"))
{
    0
} else {
    5
};

#[derive(Clone, Debug)]
pub struct ZingoConfig {
    pub server: http::Uri,
    pub chain: Network,
    pub anchor_offset: [u32; 5],
    pub monitor_mempool: bool,
    pub data_dir: Option<String>,
}

impl ZingoConfig {
    // Create an unconnected (to any server) config to test for local wallet etc...
    pub fn create_unconnected(chain: Network, dir: Option<String>) -> ZingoConfig {
        ZingoConfig {
            server: http::Uri::default(),
            chain,
            monitor_mempool: false,
            anchor_offset: [4u32; 5],
            data_dir: dir,
        }
    }

    //Convenience wrapper
    pub fn sapling_activation_height(&self) -> u64 {
        self.chain
            .activation_height(NetworkUpgrade::Sapling)
            .unwrap()
            .into()
    }

    pub fn set_data_dir(&mut self, dir_str: String) {
        self.data_dir = Some(dir_str);
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

    pub fn get_zcash_data_path(&self) -> Box<Path> {
        if cfg!(target_os = "ios") || cfg!(target_os = "android") {
            PathBuf::from(&self.data_dir.as_ref().unwrap()).into_boxed_path()
        } else {
            let mut zcash_data_location;
            // If there's some --data-dir path provided, use it
            if self.data_dir.is_some() {
                zcash_data_location = PathBuf::from(&self.data_dir.as_ref().unwrap());
            } else {
                if cfg!(target_os = "macos") || cfg!(target_os = "windows") {
                    zcash_data_location =
                        dirs::data_dir().expect("Couldn't determine app data directory!");
                    zcash_data_location.push("Zcash");
                } else {
                    if dirs::home_dir().is_none() {
                        info!("Couldn't determine home dir!");
                    }
                    zcash_data_location =
                        dirs::home_dir().expect("Couldn't determine home directory!");
                    zcash_data_location.push(".zcash");
                };

                match &self.chain {
                    Network::Testnet => zcash_data_location.push("testnet3"),
                    Network::Regtest => zcash_data_location.push("regtest"),
                    Network::Mainnet => {}
                    Network::FakeMainnet => zcash_data_location.push("fakemainnet"),
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
        if cfg!(target_os = "ios") || cfg!(target_os = "android") {
            Ok(PathBuf::from(&self.data_dir.as_ref().unwrap()).into_boxed_path())
        } else {
            if dirs::home_dir().is_none() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Couldn't determine Home Dir",
                ));
            }

            let mut zcash_params = self.get_zcash_data_path().into_path_buf();
            zcash_params.push("..");
            if cfg!(target_os = "macos") || cfg!(target_os = "windows") {
                zcash_params.push("ZcashParams");
            } else {
                zcash_params.push(".zcash-params");
            }

            match std::fs::create_dir_all(zcash_params.clone()) {
                Ok(_) => Ok(zcash_params.into_boxed_path()),
                Err(e) => {
                    eprintln!("Couldn't create zcash params directory\n{}", e);
                    Err(e)
                }
            }
        }
    }

    pub fn get_wallet_path(&self) -> Box<Path> {
        let mut wallet_location = self.get_zcash_data_path().into_path_buf();
        wallet_location.push(WALLET_NAME);

        wallet_location.into_boxed_path()
    }

    pub fn wallet_exists(&self) -> bool {
        return self.get_wallet_path().exists();
    }

    pub fn backup_existing_wallet(&self) -> Result<String, String> {
        if !self.wallet_exists() {
            return Err(format!(
                "Couldn't find existing wallet to backup. Looked in {:?}",
                self.get_wallet_path().to_str()
            ));
        }
        use std::time::{SystemTime, UNIX_EPOCH};

        let mut backup_file_path = self.get_zcash_data_path().into_path_buf();
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
        let mut log_path = self.get_zcash_data_path().into_path_buf();
        log_path.push(LOGFILE_NAME);
        //println!("LogFile:\n{}", log_path.to_str().unwrap());

        log_path.into_boxed_path()
    }

    pub fn get_server_or_default(server: Option<String>) -> http::Uri {
        match server {
            Some(s) => {
                let mut s = if s.starts_with("http") {
                    s
                } else {
                    "http://".to_string() + &s
                };
                let uri: http::Uri = s.parse().unwrap();
                if uri.port().is_none() {
                    s = s + ":9067";
                }
                s
            }
            None => DEFAULT_SERVER.to_string(),
        }
        .parse()
        .unwrap()
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
            Network::Testnet | Network::Regtest | Network::FakeMainnet => [0xEF],
            Network::Mainnet => [0x80],
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Network {
    Testnet,
    Regtest,
    Mainnet,
    FakeMainnet,
}

impl Network {
    pub fn hrp_orchard_spending_key(&self) -> &str {
        match self {
            Network::Testnet => "secret-orchard-sk-test",
            Network::Regtest => "secret-orchard-sk-regtest",
            Network::Mainnet => "secret-orchard-sk-main",
            Network::FakeMainnet => "secret-orchard-sk-main",
        }
    }
    pub fn hrp_unified_full_viewing_key(&self) -> &str {
        match self {
            Network::Testnet => "uviewtest",
            Network::Regtest => "uviewregtest",
            Network::Mainnet => "uview",
            Network::FakeMainnet => "uview",
        }
    }
    pub fn to_zcash_address_network(&self) -> zcash_address::Network {
        self.address_network().unwrap()
    }
}

impl std::fmt::Display for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use Network::*;
        let name = match self {
            Testnet => "test",
            Regtest => "regtest",
            Mainnet => "main",
            FakeMainnet => "fakemainnet",
        };
        write!(f, "{name}")
    }
}

impl Parameters for Network {
    fn activation_height(
        &self,
        nu: NetworkUpgrade,
    ) -> Option<zcash_primitives::consensus::BlockHeight> {
        use Network::*;
        match self {
            Mainnet => MAIN_NETWORK.activation_height(nu),
            Testnet => TEST_NETWORK.activation_height(nu),
            FakeMainnet => Some(BlockHeight::from_u32(1)),
            Regtest => Some(BlockHeight::from_u32(1)),
        }
    }

    fn coin_type(&self) -> u32 {
        use Network::*;
        match self {
            Testnet => constants::testnet::COIN_TYPE,
            Regtest => constants::regtest::COIN_TYPE,
            Mainnet => constants::mainnet::COIN_TYPE,
            FakeMainnet => constants::mainnet::COIN_TYPE,
        }
    }

    fn hrp_sapling_extended_spending_key(&self) -> &str {
        use Network::*;
        match self {
            Testnet => constants::testnet::HRP_SAPLING_EXTENDED_SPENDING_KEY,
            Regtest => constants::regtest::HRP_SAPLING_EXTENDED_SPENDING_KEY,
            Mainnet => constants::mainnet::HRP_SAPLING_EXTENDED_SPENDING_KEY,
            FakeMainnet => constants::mainnet::HRP_SAPLING_EXTENDED_SPENDING_KEY,
        }
    }

    fn hrp_sapling_extended_full_viewing_key(&self) -> &str {
        use Network::*;
        match self {
            Testnet => constants::testnet::HRP_SAPLING_EXTENDED_FULL_VIEWING_KEY,
            Regtest => constants::regtest::HRP_SAPLING_EXTENDED_FULL_VIEWING_KEY,
            Mainnet => constants::mainnet::HRP_SAPLING_EXTENDED_FULL_VIEWING_KEY,
            FakeMainnet => constants::mainnet::HRP_SAPLING_EXTENDED_FULL_VIEWING_KEY,
        }
    }

    fn hrp_sapling_payment_address(&self) -> &str {
        use Network::*;
        match self {
            Testnet => constants::testnet::HRP_SAPLING_PAYMENT_ADDRESS,
            Regtest => constants::regtest::HRP_SAPLING_PAYMENT_ADDRESS,
            Mainnet => constants::mainnet::HRP_SAPLING_PAYMENT_ADDRESS,
            FakeMainnet => constants::mainnet::HRP_SAPLING_PAYMENT_ADDRESS,
        }
    }

    fn b58_pubkey_address_prefix(&self) -> [u8; 2] {
        use Network::*;
        match self {
            Testnet => constants::testnet::B58_PUBKEY_ADDRESS_PREFIX,
            Regtest => constants::regtest::B58_PUBKEY_ADDRESS_PREFIX,
            Mainnet => constants::mainnet::B58_PUBKEY_ADDRESS_PREFIX,
            FakeMainnet => constants::mainnet::B58_PUBKEY_ADDRESS_PREFIX,
        }
    }

    fn b58_script_address_prefix(&self) -> [u8; 2] {
        use Network::*;
        match self {
            Testnet => constants::testnet::B58_SCRIPT_ADDRESS_PREFIX,
            Regtest => constants::regtest::B58_SCRIPT_ADDRESS_PREFIX,
            Mainnet => constants::mainnet::B58_SCRIPT_ADDRESS_PREFIX,
            FakeMainnet => constants::mainnet::B58_SCRIPT_ADDRESS_PREFIX,
        }
    }

    fn address_network(&self) -> Option<zcash_address::Network> {
        Some(match self {
            Network::Testnet => zcash_address::Network::Test,
            Network::Regtest => zcash_address::Network::Regtest,
            _ => zcash_address::Network::Main,
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        let result = 2 + 2;
        assert_eq!(result, 4);
    }
}

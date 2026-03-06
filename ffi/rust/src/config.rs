use std::num::NonZeroU32;

use pepper_sync::config::PerformanceLevel;
#[cfg(feature = "regtest")]
use zingo_common_components::protocol::activation_heights::for_test::all_height_one_nus;
use zingolib::{
    config::{
        ChainType, SyncConfig, TransparentAddressDiscovery, ZingoConfig, construct_lightwalletd_uri,
    },
    wallet::WalletSettings,
};

use crate::{Chain, Performance, error::WalletError};

pub fn chain_to_chaintype(chain: Chain) -> Result<ChainType, WalletError> {
    match chain {
        Chain::Mainnet => Ok(ChainType::Mainnet),
        Chain::Testnet => Ok(ChainType::Testnet),
        #[cfg(feature = "regtest")]
        Chain::Regtest => Ok(ChainType::Regtest(all_height_one_nus())),
        #[cfg(not(feature = "regtest"))]
        Chain::Regtest => Err(WalletError::Internal(
            "regtest feature not enabled".into(),
        )),
    }
}

pub fn perf_to_level(p: Performance) -> PerformanceLevel {
    match p {
        Performance::Maximum => PerformanceLevel::Maximum,
        Performance::High => PerformanceLevel::High,
        Performance::Medium => PerformanceLevel::Medium,
        Performance::Low => PerformanceLevel::Low,
    }
}

pub fn construct_config(
    indexer_uri: String,
    chain: Chain,
    perf: Performance,
    min_confirmations: u32,
) -> Result<(ZingoConfig, http::Uri), WalletError> {
    let lightwalletd_uri = construct_lightwalletd_uri(Some(indexer_uri));

    let min_conf = NonZeroU32::try_from(min_confirmations)
        .map_err(|_| WalletError::Internal("min_confirmations must be >= 1".into()))?;

    let config = zingolib::config::load_clientconfig(
        lightwalletd_uri.clone(),
        None,
        chain_to_chaintype(chain)?,
        WalletSettings {
            sync_config: SyncConfig {
                transparent_address_discovery: TransparentAddressDiscovery::minimal(),
                performance_level: perf_to_level(perf),
            },
            min_confirmations: min_conf,
        },
        NonZeroU32::try_from(1).expect("hard-coded integer"),
        "".to_string(),
    )
    .map_err(|e| WalletError::Internal(format!("Config load error: {e}")))?;

    Ok((config, lightwalletd_uri))
}

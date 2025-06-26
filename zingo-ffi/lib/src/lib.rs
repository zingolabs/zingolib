uniffi::include_scaffolding!("zingo");

#[macro_use]
extern crate lazy_static;
extern crate android_logger;

#[cfg(target_os = "android")]
use android_logger::{Config, FilterBuilder};
#[cfg(target_os = "android")]
use log::Level;

use std::num::NonZeroU32;
use std::str::FromStr;
use std::sync::RwLock;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bip0039::Mnemonic;
use json::object;
use rustls::crypto::{CryptoProvider, ring::default_provider};

use zcash_address::unified::{Container, Encoding, Ufvk};
use zcash_keys::address::Address;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::zip32::AccountId;
use zcash_protocol::consensus::NetworkType;

use pepper_sync::keys::transparent;
use pepper_sync::config::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery};
use pepper_sync::wallet::{KeyIdInterface, SyncMode};
use zingolib::commands::RT;
use zingolib::config::{ChainType, RegtestNetwork, ZingoConfig, construct_lightwalletd_uri};
use zingolib::data::PollReport;
use zingolib::lightclient::LightClient;
use zingolib::utils::{conversion::address_from_str, conversion::txid_from_hex_encoded_str};
use zingolib::wallet::keys::{
    WalletAddressRef,
    unified::{ReceiverSelection, UnifiedKeyStore},
};
use zingolib::wallet::{LightWallet, WalletBase, WalletSettings};

// We'll use a RwLock to store a global lightclient instance,
// so we don't have to keep creating it. We need to store it here, in rust
// because we can't return such a complex structure back to JS
lazy_static! {
    static ref LIGHTCLIENT: RwLock<Option<LightClient>> = RwLock::new(None);
}

fn store_client(lightclient: LightClient) {
    LIGHTCLIENT.write().unwrap().replace(lightclient);
}

fn construct_uri_load_config(
    uri: String,
    chain_hint: String,
) -> Result<(ZingoConfig, http::Uri), String> {
    // if uri is empty -> Offline Mode.
    let lightwalletd_uri = construct_lightwalletd_uri(Some(uri));

    let chaintype = match chain_hint.as_str() {
        "main" => ChainType::Mainnet,
        "test" => ChainType::Testnet,
        "regtest" => ChainType::Regtest(RegtestNetwork::all_upgrades_active()),
        _ => return Err("Error: Not a valid chain hint!".to_string()),
    };
    let config = match zingolib::config::load_clientconfig(
        lightwalletd_uri.clone(),
        None,
        chaintype,
        WalletSettings {
            sync_config: SyncConfig {
                transparent_address_discovery: TransparentAddressDiscovery::minimal(),
                performance_level: PerformanceLevel::Medium,
            },
            min_confirmations: NonZeroU32::try_from(3).unwrap(),
        },
        NonZeroU32::try_from(1).expect("hard-coded integer"),
    ) {
        Ok(c) => c,
        Err(e) => {
            return Err(format!("Error: Config load: {e}"));
        }
    };

    Ok((config, lightwalletd_uri))
}

pub fn init_logging() -> String {
    // this is only for Android
    #[cfg(target_os = "android")]
    android_logger::init_once(
        Config::default().with_min_level(Level::Trace).with_filter(
            FilterBuilder::new()
                .parse("debug,hello::crate=zingolib")
                .build(),
        ),
    );

    "OK".to_string()
}

pub fn init_new(server_uri: String, chain_hint: String) -> String {
    let (config, lightwalletd_uri) = match construct_uri_load_config(server_uri, chain_hint) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let latest_block_height = match RT
        .block_on(async move { zingolib::grpc_connector::get_latest_block(lightwalletd_uri).await })
    {
        Ok(block_id) => block_id.height,
        Err(e) => {
            return format!("Error: {e}");
        }
    };
    let lightclient = match LightClient::new(
        config,
        (latest_block_height.saturating_sub(100) as u32).into(),
        false,
    ) {
        Ok(l) => l,
        Err(e) => {
            return format!("Error: {e}");
        }
    };
    store_client(lightclient);

    get_seed()
}

// TODO: change `seed` to `seed_phrase` or `mnemonic_phrase`
pub fn init_from_seed(
    server_uri: String,
    seed: String,
    birthday: u64,
    chain_hint: String,
) -> String {
    let (config, _lightwalletd_uri) = match construct_uri_load_config(server_uri, chain_hint) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let mnemonic = match Mnemonic::from_phrase(seed) {
        Ok(m) => m,
        Err(e) => {
            return format!("Error: {e}");
        }
    };
    let wallet = match LightWallet::new(
        config.chain,
        WalletBase::Mnemonic {
            mnemonic,
            no_of_accounts: config.no_of_accounts,
        },
        BlockHeight::from_u32(birthday as u32),
        WalletSettings {
            sync_config: SyncConfig {
                transparent_address_discovery: TransparentAddressDiscovery::minimal(),
                performance_level: PerformanceLevel::Medium,
            },
            min_confirmations: NonZeroU32::try_from(3).unwrap(),
        },
    ) {
        Ok(w) => w,
        Err(e) => return format!("Error: {e}"),
    };
    let lightclient = match LightClient::create_from_wallet(wallet, config, false) {
        Ok(l) => l,
        Err(e) => {
            return format!("Error: {e}");
        }
    };
    store_client(lightclient);

    get_seed()
}

pub fn init_from_ufvk(
    server_uri: String,
    ufvk: String,
    birthday: u64,
    chain_hint: String,
) -> String {
    let (config, _lightwalletd_uri) = match construct_uri_load_config(server_uri, chain_hint) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let wallet = match LightWallet::new(
        config.chain,
        WalletBase::Ufvk(ufvk),
        BlockHeight::from_u32(birthday as u32),
        WalletSettings {
            sync_config: SyncConfig {
                transparent_address_discovery: TransparentAddressDiscovery::minimal(),
                performance_level: PerformanceLevel::Medium,
            },
            min_confirmations: NonZeroU32::try_from(3).unwrap(),
        },
    ) {
        Ok(w) => w,
        Err(e) => return format!("Error: {e}"),
    };
    let lightclient = match LightClient::create_from_wallet(wallet, config, false) {
        Ok(l) => l,
        Err(e) => {
            return format!("Error: {e}");
        }
    };
    store_client(lightclient);

    get_ufvk()
}

pub fn init_from_b64(server_uri: String, base64_data: String, chain_hint: String) -> String {
    let (config, _lightwalletd_uri) = match construct_uri_load_config(server_uri, chain_hint) {
        Ok(c) => c,
        Err(e) => return e,
    };
    let decoded_bytes = match STANDARD.decode(&base64_data) {
        Ok(b) => b,
        Err(e) => {
            return format!(
                "Error: Decoding Base64: {}, Size: {}, Content: {}",
                e,
                base64_data.len(),
                base64_data
            );
        }
    };

    let wallet = match LightWallet::read(&decoded_bytes[..], config.chain) {
        Ok(w) => w,
        Err(e) => return format!("Error: {e}"),
    };
    let has_seed = wallet.mnemonic().is_some();
    let lightclient = match LightClient::create_from_wallet(wallet, config, false) {
        Ok(l) => l,
        Err(e) => {
            return format!("Error: {e}");
        }
    };
    store_client(lightclient);

    if has_seed { get_seed() } else { get_ufvk() }
}

pub fn save_to_b64() -> String {
    // Return the wallet as a base64 encoded string
    if let Some(lightclient) = &mut *LIGHTCLIENT.write().unwrap() {
        // we need to use STANDARD because swift is expecting the encoded String with padding
        // I tried with STANDARD_NO_PAD and the decoding return `nil`.
        RT.block_on(async move {
            match lightclient.wallet.write().await.save() {
                Ok(Some(wallet_bytes)) => STANDARD.encode(wallet_bytes),
                // TODO: check this is better than a custom error when save is not required (empty buffer)
                Ok(None) => "Error: No need to save the wallet file".to_string(),
                Err(e) => format!("Error: {e}"),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

// TODO: deprecate
pub fn execute_command(cmd: String, args_list: String) -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.write().unwrap() {
        let args = if args_list.is_empty() {
            vec![]
        } else {
            vec![args_list.as_ref()]
        };
        zingolib::commands::do_user_command(&cmd, &args, lightclient)
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn get_latest_block_server(server_uri: String) -> String {
    let lightwalletd_uri: http::Uri = match server_uri.parse() {
        Ok(uri) => uri,
        Err(e) => {
            return format!("Error: failed to parse uri. {e}");
        }
    };
    match RT
        .block_on(async move { zingolib::grpc_connector::get_latest_block(lightwalletd_uri).await })
    {
        Ok(block_id) => block_id.height.to_string(),
        Err(e) => format!("Error: {e}"),
    }
}

pub fn get_latest_block_wallet() -> String {
    if let Some(lightclient) = &*LIGHTCLIENT.read().unwrap() {
        RT.block_on(async move {
            object! { "height" => json::JsonValue::from(lightclient.wallet.write().await.sync_state.wallet_height().map(u32::from).unwrap_or(0))}.pretty(2)
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn get_developer_donation_address() -> String {
    zingolib::config::DEVELOPER_DONATION_ADDRESS.to_string()
}

pub fn get_zennies_for_zingo_donation_address() -> String {
    zingolib::config::ZENNIES_FOR_ZINGO_DONATION_ADDRESS.to_string()
}

pub fn get_value_transfers() -> String {
    if let Some(lightclient) = &*LIGHTCLIENT.read().unwrap() {
        RT.block_on(async move {
            match lightclient
                .wallet
                .read()
                .await
                .value_transfers(true)
                .await
            {
                Ok(value_transfers) => json::JsonValue::from(value_transfers).pretty(2),
                Err(e) => format!("Error: {e}"),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn set_crypto_default_provider_to_ring() -> String {
    CryptoProvider::get_default().map_or_else(
        || match default_provider().install_default() {
            Ok(_) => "true".to_string(),
            Err(_) => "Error: Failed to install crypto provider".to_string(),
        },
        |_| "true".to_string(),
    )
}

pub fn poll_sync() -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.write().unwrap() {
        match lightclient.poll_sync() {
            PollReport::NoHandle => "Sync task has not been launched.".to_string(),
            PollReport::NotReady => "Sync task is not complete.".to_string(),
            PollReport::Ready(result) => match result {
                Ok(sync_result) => {
                    json::object! { "sync_complete" => json::JsonValue::from(sync_result) }
                        .pretty(2)
                }
                Err(e) => format!("Error: {e}"),
            },
        }
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn run_sync() -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.write().unwrap() {
        if lightclient.sync_mode() == SyncMode::Paused {
            lightclient.resume_sync().expect("sync should be paused");
            "Resuming sync task...".to_string()
        } else {
            RT.block_on(async move {
                match lightclient.sync().await {
                    Ok(_) => "Launching sync task...".to_string(),
                    Err(e) => format!("Error: {e}"),
                }
            })
        }
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn pause_sync() -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.write().unwrap() {
        match lightclient.pause_sync() {
            Ok(_) => "Pausing sync task...".to_string(),
            Err(e) => format!("Error: {e}"),
        }
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn stop_sync() -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.write().unwrap() {
        match lightclient.stop_sync() {
            Ok(_) => "Stopping sync task...".to_string(),
            Err(e) => format!("Error: {e}"),
        }
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn status_sync() -> String {
    if let Some(lightclient) = &*LIGHTCLIENT.read().unwrap() {
        RT.block_on(async move {
            match pepper_sync::sync_status(&*lightclient.wallet.read().await).await {
                Ok(status) => json::JsonValue::from(status).pretty(2),
                Err(e) => format!("Error: {e}"),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn run_rescan() -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.write().unwrap() {
        RT.block_on(async move {
            match lightclient.rescan().await {
                Ok(_) => "Launching rescan...".to_string(),
                Err(e) => format!("Error: {e}"),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn info_server() -> String {
    if let Some(lightclient) = &*LIGHTCLIENT.read().unwrap() {
        RT.block_on(async move { lightclient.do_info().await })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

// TODO: rename "get_seed_phrase" or "get_mnemonic_phrase"
// or if other recovery info is being used could rename "get_recovery_info" ?
pub fn get_seed() -> String {
    if let Some(lightclient) = &*LIGHTCLIENT.read().unwrap() {
        RT.block_on(async move {
            match lightclient.wallet.read().await.recovery_info() {
                Some(recovery_info) => serde_json::to_string_pretty(&recovery_info)
                    .unwrap_or_else(|_| "error: get seed. failed to serialize".to_string()),
                None => "error: get seed. no mnemonic found. wallet loaded from key.".to_string(),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn get_ufvk() -> String {
    if let Some(lightclient) = &*LIGHTCLIENT.read().unwrap() {
        RT.block_on(async move {
            let wallet = lightclient.wallet.read().await;
            let ufvk: UnifiedFullViewingKey = match wallet
                .unified_key_store
                .get(&AccountId::ZERO)
                .expect("account 0 must always exist")
                .try_into()
            {
                Ok(ufvk) => ufvk,
                Err(e) => {
                    return format!("Error: {e}");
                }
            };
            object! {
                "ufvk" => ufvk.encode(&wallet.network),
                "birthday" => u32::from(wallet.birthday)
            }
            .pretty(2)
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn change_server(server_uri: String) -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.write().unwrap() {
        if server_uri.is_empty() {
            lightclient.set_server(http::Uri::default());
            "server set (default)".to_string()
        } else {
            match http::Uri::from_str(&server_uri) {
                Ok(uri) => {
                    lightclient.set_server(uri);
                    "server set".to_string()
                }
                Err(_) => "Error: invalid server uri".to_string(),
            }
        }
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn wallet_kind() -> String {
    if let Some(lightclient) = &*LIGHTCLIENT.read().unwrap() {
        RT.block_on(async move {
            let wallet = lightclient.wallet.read().await;
            if wallet.mnemonic().is_some() {
                object! {"kind" => "Loaded from seed or mnemonic phrase)",
                        "transparent" => true,
                        "sapling" => true,
                        "orchard" => true,
                }
                .pretty(2)
            } else {
                match wallet
                    .unified_key_store
                    .get(&AccountId::ZERO)
                    .expect("account 0 must always exist")
                {
                    UnifiedKeyStore::Spend(_) => object! {
                        "kind" => "Loaded from unified spending key",
                        "transparent" => true,
                        "sapling" => true,
                        "orchard" => true,
                    }
                    .pretty(2),
                    UnifiedKeyStore::View(ufvk) => object! {
                        "kind" => "Loaded from unified full viewing key",
                        "transparent" => ufvk.transparent().is_some(),
                        "sapling" => ufvk.sapling().is_some(),
                        "orchard" => ufvk.orchard().is_some(),
                    }
                    .pretty(2),
                    UnifiedKeyStore::Empty => object! {
                        "kind" => "No keys found",
                        "transparent" => false,
                        "sapling" => false,
                        "orchard" => false,
                    }
                    .pretty(2),
                }
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn parse_address(address: String) -> String {
    if address.is_empty() {
        "Error: The address is empty".to_string()
    } else {
        fn make_decoded_chain_pair(
            address: &str,
        ) -> Option<(zcash_client_backend::address::Address, ChainType)> {
            [
                ChainType::Mainnet,
                ChainType::Testnet,
                ChainType::Regtest(RegtestNetwork::all_upgrades_active()),
            ]
            .iter()
            .find_map(|chain| Address::decode(chain, address).zip(Some(*chain)))
        }
        if let Some((recipient_address, chain_name)) = make_decoded_chain_pair(&address) {
            let chain_name_string = match chain_name {
                ChainType::Mainnet => "main",
                ChainType::Testnet => "test",
                ChainType::Regtest(_) => "regtest",
            };
            match recipient_address {
                Address::Sapling(_) => object! {
                    "status" => "success",
                    "chain_name" => chain_name_string,
                    "address_kind" => "sapling",
                }
                .pretty(2),
                Address::Transparent(_) => object! {
                    "status" => "success",
                    "chain_name" => chain_name_string,
                    "address_kind" => "transparent",
                }
                .pretty(2),
                Address::Tex(_) => object! {
                    "status" => "success",
                    "chain_name" => chain_name_string,
                    "address_kind" => "tex",
                }
                .pretty(2),
                Address::Unified(ua) => {
                    let mut receivers_available = vec![];
                    if ua.sapling().is_some() {
                        receivers_available.push("sapling")
                    }
                    if ua.transparent().is_some() {
                        receivers_available.push("transparent")
                    }
                    if ua.orchard().is_some() {
                        receivers_available.push("orchard");
                        object! {
                            "status" => "success",
                            "chain_name" => chain_name_string,
                            "address_kind" => "unified",
                            "receivers_available" => receivers_available,
                            "only_orchard_ua" => zcash_keys::address::UnifiedAddress::from_receivers(ua.orchard().cloned(), None, None).expect("To construct UA").encode(&chain_name),
                        }
                        .pretty(2)
                    } else {
                        object! {
                            "status" => "success",
                            "chain_name" => chain_name_string,
                            "address_kind" => "unified",
                            "receivers_available" => receivers_available,
                        }
                        .pretty(2)
                    }
                }
            }
        } else {
            object! {
                "status" => "Invalid address",
                "chain_name" => json::JsonValue::Null,
                "address_kind" => json::JsonValue::Null,
            }
            .pretty(2)
        }
    }
}

pub fn parse_ufvk(ufvk: String) -> String {
    if ufvk.is_empty() {
        "Error: The ufvk is empty".to_string()
    } else {
        json::stringify_pretty(
            match Ufvk::decode(&ufvk) {
                Ok((network, ufvk)) => {
                    let mut pools_available = vec![];
                    for fvk in ufvk.items_as_parsed() {
                        match fvk {
                            zcash_address::unified::Fvk::Orchard(_) => {
                                pools_available.push("orchard")
                            }
                            zcash_address::unified::Fvk::Sapling(_) => {
                                pools_available.push("sapling")
                            }
                            zcash_address::unified::Fvk::P2pkh(_) => {
                                pools_available.push("transparent")
                            }
                            zcash_address::unified::Fvk::Unknown { .. } => pools_available.push(
                                "Error: Unknown future protocol. Perhaps you're using old software",
                            ),
                        }
                    }
                    object! {
                        "status" => "success",
                        "chain_name" => match network {
                            NetworkType::Main => "main",
                            NetworkType::Test => "test",
                            NetworkType::Regtest => "regtest",
                        },
                        "address_kind" => "ufvk",
                        "pools_available" => pools_available,
                    }
                }
                Err(_) => {
                    object! {
                        "status" => "Invalid viewkey",
                        "chain_name" => json::JsonValue::Null,
                        "address_kind" => json::JsonValue::Null
                    }
                }
            },
            2,
        )
    }
}

pub fn get_version() -> String {
    zingolib::git_description().to_string()
}

pub fn get_messages(address: String) -> String {
    if let Some(lightclient) = &*LIGHTCLIENT.read().unwrap() {
        RT.block_on(async move {
            match lightclient
                .messages_containing(Some(address.as_str()))
                .await
            {
                Ok(value_transfers) => json::JsonValue::from(value_transfers).pretty(2),
                Err(e) => format!("Error: {e}"),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn get_balance() -> String {
    if let Some(lightclient) = &*LIGHTCLIENT.read().unwrap() {
        RT.block_on(async move {
            match lightclient
                .account_balance(AccountId::ZERO)
                .await
            {
                Ok(bal) => json::JsonValue::from(bal).pretty(2),
                Err(e) => format!("Error: {e}"),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn get_total_memobytes_to_address() -> String {
    if let Some(lightclient) = &*LIGHTCLIENT.read().unwrap() {
        RT.block_on(async move {
            match lightclient.do_total_memobytes_to_address().await {
                Ok(total_memo_bytes) => json::JsonValue::from(total_memo_bytes).pretty(2),
                Err(e) => format!("Error: {e}"),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn get_total_value_to_address() -> String {
    if let Some(lightclient) = &*LIGHTCLIENT.read().unwrap() {
        RT.block_on(async move {
            match lightclient.do_total_value_to_address().await {
                Ok(total_values) => json::JsonValue::from(total_values).pretty(2),
                Err(e) => format!("Error: {e}"),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn get_total_spends_to_address() -> String {
    if let Some(lightclient) = &*LIGHTCLIENT.read().unwrap() {
        RT.block_on(async move {
            match lightclient.do_total_spends_to_address().await {
                Ok(total_spends) => json::JsonValue::from(total_spends).pretty(2),
                Err(e) => format!("Error: {e}"),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn zec_price(tor: String) -> String {
    if let Some(lightclient) = &*LIGHTCLIENT.read().unwrap() {
        RT.block_on(async move {
            let Ok(tor_bool) = tor.parse() else {
                return "Error: failed to parse tor setting.".to_string();
            };
            let client_check = match (tor_bool, lightclient.tor_client()) {
                (true, Some(tc)) => Ok(Some(tc)),
                (true, None) => Err(()),
                (false, _) => Ok(None),
            };
            let tor_client = match client_check {
                Ok(tc) => tc,
                Err(_) => {
                    return "Error: no tor client found. please create a tor client.".to_string();
                }
            };

            match lightclient
                .wallet
                .write()
                .await
                .update_current_price(tor_client)
                .await
            {
                Ok(price) => object! { "current_price" => price }.pretty(2),
                Err(e) => format!("Error: {e}"),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn resend_transaction(txid: String) -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.write().unwrap() {
        let txid = match txid_from_hex_encoded_str(&txid) {
            Ok(txid) => txid,
            Err(e) => return format!("Error: {e}"),
        };

        RT.block_on(async move {
            match lightclient.resend(txid).await {
                Ok(_) => "Successfully resent transaction.".to_string(),
                Err(e) => format!("Error: {e}"),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn remove_transaction(txid: String) -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.write().unwrap() {
        let txid = match txid_from_hex_encoded_str(&txid) {
            Ok(txid) => txid,
            Err(e) => return format!("Error: {e}"),
        };

        RT.block_on(async move {
            match lightclient
                .wallet
                .write()
                .await
                .remove_unconfirmed_transaction(txid)
            {
                Ok(_) => "Successfully removed transaction.".to_string(),
                Err(e) => format!("Error: {e}"),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

// we don't use this anymore...
pub fn get_spendable_balance_with_address(address: String, zennies: String) -> String {
    if let Some(lightclient) = &*LIGHTCLIENT.read().unwrap() {
        let Ok(address) = address_from_str(&address) else {
            return "Error: unknown address format".to_string();
        };
        let Ok(zennies) = zennies.parse() else {
            return "Error: failed to parse zennies setting.".to_string();
        };
        RT.block_on(async move {
            match lightclient
                .max_send_value(address, zennies, AccountId::ZERO)
                .await
            {
                Ok(bal) => {
                    object! { "spendable_balance" => bal.into_u64() }.pretty(2)
                }
                Err(e) => format!("error: {e}"),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn get_spendable_balance_total() -> String {
    if let Some(lightclient) = &*LIGHTCLIENT.read().unwrap() {
        RT.block_on(async move {
            let wallet = lightclient.wallet.write().await;
            let spendable_balance =
                match wallet.shielded_spendable_balance(AccountId::ZERO, false) {
                    Ok(bal) => bal,
                    Err(e) => return format!("Error: {e}"),
                };
            object! {
                "spendable_balance" => spendable_balance.into_u64(),
            }
            .pretty(2)
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn set_option_wallet() -> String {
    "Error: unimplemented".to_string()
}

pub fn get_option_wallet() -> String {
    "Error: unimplemented".to_string()
}

pub fn create_tor_client(data_dir: String) -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.write().unwrap() {
        if lightclient.tor_client.is_some() {
            return "Error: Tor client already exists.".to_string();
        }

        match RT.block_on(async move {
            lightclient.create_tor_client(Some(data_dir.into())).await
        }) {
            Ok(_) => "Successfully created tor client.".to_string(),
            Err(e) => format!("Error creating tor client: {e}"),
        }
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn remove_tor_client() -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.write().unwrap() {
        if lightclient.tor_client.is_none() {
            return "Error: Tor client is not active.".to_string();
        }

        RT.block_on(async move {
            lightclient.remove_tor_client().await;
        });

        "Successfully removed tor client.".to_string()
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn get_unified_addresses() -> String {
    if let Some(lightclient) = &*LIGHTCLIENT.read().unwrap() {
        RT.block_on(async move { lightclient.unified_addresses_json().await.pretty(2) })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn get_transparent_addresses() -> String {
    if let Some(lightclient) = &*LIGHTCLIENT.read().unwrap() {
        RT.block_on(async move { lightclient.transparent_addresses_json().await.pretty(2) })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn create_new_unified_address(receivers: String) -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.write().unwrap() {
        RT.block_on(async move {
            let mut wallet = lightclient.wallet.write().await;
            let network = wallet.network;
            let receivers_available = ReceiverSelection {
                orchard: receivers.contains('o'),
                sapling: receivers.contains('z'),
            };
            match wallet.generate_unified_address(receivers_available, AccountId::ZERO) {
                Ok((id, unified_address)) => {
                    json::object! {
                        "account" => u32::from(AccountId::ZERO),
                        "address_index" => id.address_index,
                        "has_orchard" => unified_address.has_orchard(),
                        "has_sapling" => unified_address.has_sapling(),
                        "has_transparent" => unified_address.has_transparent(),
                        "encoded_address" => unified_address.encode(&network),
                    }.pretty(2)
                }
                Err(e) => format!("Error: {e}"),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn create_new_transparent_address() -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.write().unwrap() {
        RT.block_on(async move {
            let mut wallet = lightclient.wallet.write().await;
            let network = wallet.network;
            match wallet.generate_transparent_address(AccountId::ZERO, true) {
                Ok((id, transparent_address)) => {
                    json::object! {
                        "account" => u32::from(id.account_id()),
                        "address_index" => id.address_index().index(),
                        "scope" => id.scope().to_string(),
                        "encoded_address" => transparent::encode_address(&network,  transparent_address),
                    }.pretty(2)
                }
                Err(e) => format!("Error: {e}"),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn check_my_address(address: String) -> String {
    if let Some(lightclient) = &*LIGHTCLIENT.read().unwrap() {
        RT.block_on(async move {
            match lightclient.wallet.read().await.is_wallet_address(&address) {
                Ok(address_ref) => address_ref.map_or(
                    json::object! { "is_wallet_address" => false },
                    |address_ref| match address_ref {
                        WalletAddressRef::Unified {
                            account_id,
                            address_index,
                            has_orchard,
                            has_sapling,
                            has_transparent,
                            encoded_address,
                        } => json::object! {
                            "is_wallet_address" => true,
                            "address_type" => "unified".to_string(),
                            "address_index" => address_index,
                            "account_id" => u32::from(account_id),
                            "has_orchard" => has_orchard,
                            "has_sapling" => has_sapling,
                            "has_transparent" => has_transparent,
                            "encoded_address" => encoded_address,
                        },
                        WalletAddressRef::OrchardInternal {
                            account_id,
                            diversifier_index,
                            encoded_address,
                        } => json::object! {
                            "is_wallet_address" => true,
                            "address_type" => "orchard_internal".to_string(),
                            "account_id" => u32::from(account_id),
                            "diversifier_index" => u128::from(diversifier_index).to_string(),
                            "encoded_address" => encoded_address,
                        },
                        WalletAddressRef::SaplingExternal {
                            account_id,
                            diversifier_index,
                            encoded_address,
                        } => json::object! {
                            "is_wallet_address" => true,
                            "address_type" => "sapling".to_string(),
                            "account_id" => u32::from(account_id),
                            "diversifier_index" => u128::from(diversifier_index).to_string(),
                            "encoded_address" => encoded_address,
                        },
                        WalletAddressRef::Transparent {
                            account_id,
                            scope,
                            address_index,
                            encoded_address,
                        } => json::object! {
                            "is_wallet_address" => true,
                            "address_type" => "transparent".to_string(),
                            "account_id" => u32::from(account_id),
                            "scope" => scope.to_string(),
                            "address_index" => address_index.index(),
                            "encoded_address" => encoded_address,
                        },
                    },
                ).pretty(2),
                Err(e) => format!("Error: {e}"),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn get_wallet_save_required() -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.write().unwrap() {
        RT.block_on(async move {
            let wallet = lightclient.wallet.read().await;
            object! { "save_required" => wallet.save_required}.pretty(2)
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

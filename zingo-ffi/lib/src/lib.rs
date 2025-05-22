uniffi::include_scaffolding!("zingo");

#[macro_use]
extern crate lazy_static;
extern crate android_logger;

#[cfg(target_os = "android")]
use android_logger::{Config, FilterBuilder};
#[cfg(target_os = "android")]
use log::Level;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use json::object;
use pepper_sync::sync::{SyncConfig, TransparentAddressDiscovery};
use pepper_sync::wallet::SyncMode;
use rustls::crypto::ring::default_provider;
use rustls::crypto::CryptoProvider;
use std::str::FromStr;
use std::sync::Mutex;
use std::path::PathBuf;
use zcash_address::unified::{Container, Encoding, Ufvk};
use zcash_address::ZcashAddress;
use zcash_keys::address::Address;
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::consensus::BlockHeight;
use zcash_protocol::consensus::NetworkType;
use zingolib::config::{construct_lightwalletd_uri, ChainType, RegtestNetwork, ZingoConfig};
use zingolib::data::PollReport;
use zingolib::lightclient::describe::UAReceivers;
use zingolib::utils::conversion::address_from_str;
use zingolib::utils::conversion::txid_from_hex_encoded_str;
use zingolib::wallet::keys::unified::UnifiedKeyStore;
use zingolib::wallet::WalletSettings;
use zingolib::{commands, lightclient::LightClient, wallet::LightWallet, wallet::WalletBase};

// We'll use a MUTEX to store a global lightclient instance,
// so we don't have to keep creating it. We need to store it here, in rust
// because we can't return such a complex structure back to JS
lazy_static! {
    static ref LIGHTCLIENT: Mutex<Option<LightClient>> = Mutex::new(None);
}

fn lock_client_return_seed(mut lightclient: LightClient, tor_dir: PathBuf) -> String {
    let lightclient_tor = zingolib::commands::RT.block_on(async move {
        if let Err(e) = lightclient.create_tor_client(Some(tor_dir)).await {
            eprintln!("error: failed to create tor client. price updates disabled. {e}")
        }
        lightclient
    });
    LIGHTCLIENT.lock().unwrap().replace(lightclient_tor);

    get_seed()
}

fn construct_uri_load_config(
    uri: String,
    data_dir: String,
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
    let mut config = match zingolib::config::load_clientconfig(
        lightwalletd_uri.clone(),
        None,
        chaintype,
        WalletSettings {
            sync_config: SyncConfig {
                transparent_address_discovery: TransparentAddressDiscovery::minimal(),
            },
        },
    ) {
        Ok(c) => c,
        Err(e) => {
            return Err(format!("Error: Config load: {e}"));
        }
    };
    config.set_data_dir(data_dir);

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

pub fn init_new(server_uri: String, data_dir: String, chain_hint: String) -> String {
    let (config, lightwalletd_uri);
    match construct_uri_load_config(server_uri, data_dir.clone(), chain_hint) {
        Ok((c, h)) => (config, lightwalletd_uri) = (c, h),
        Err(s) => return s,
    }
    let latest_block_height = match zingolib::get_latest_block_height(lightwalletd_uri)
        .map_err(|e| format! {"Error: {e}"})
    {
        Ok(height) => height,
        Err(e) => return e,
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
    lock_client_return_seed(lightclient, data_dir.into())
}

pub fn init_from_seed(
    server_uri: String,
    seed: String,
    birthday: u64,
    data_dir: String,
    chain_hint: String,
) -> String {
    let (config, _lightwalletd_uri);
    match construct_uri_load_config(server_uri, data_dir.clone(), chain_hint) {
        Ok((c, h)) => (config, _lightwalletd_uri) = (c, h),
        Err(s) => return s,
    }

    let wallet = match LightWallet::new(
        config.chain,
        WalletBase::MnemonicPhrase(seed),
        BlockHeight::from_u32(birthday as u32),
        WalletSettings {
            sync_config: SyncConfig {
                transparent_address_discovery: TransparentAddressDiscovery::minimal(),
            },
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
    lock_client_return_seed(lightclient, data_dir.into())
}

pub fn init_from_ufvk(
    server_uri: String,
    ufvk: String,
    birthday: u64,
    data_dir: String,
    chain_hint: String,
) -> String {
    let (config, _lightwalletd_uri);
    match construct_uri_load_config(server_uri, data_dir.clone(), chain_hint) {
        Ok((c, h)) => (config, _lightwalletd_uri) = (c, h),
        Err(s) => return s,
    }

    let wallet = match LightWallet::new(
        config.chain,
        WalletBase::Ufvk(ufvk),
        BlockHeight::from_u32(birthday as u32),
        WalletSettings {
            sync_config: SyncConfig {
                transparent_address_discovery: TransparentAddressDiscovery::minimal(),
            },
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
    lock_client_return_seed(lightclient, data_dir.into())
}

pub fn init_from_b64(
    server_uri: String,
    base64_data: String,
    data_dir: String,
    chain_hint: String,
) -> String {
    let (config, _lightwalletd_uri);
    match construct_uri_load_config(server_uri, data_dir.clone(), chain_hint) {
        Ok((c, h)) => (config, _lightwalletd_uri) = (c, h),
        Err(s) => return s,
    }
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
    let lightclient = match LightClient::create_from_wallet(wallet, config, false) {
        Ok(l) => l,
        Err(e) => {
            return format!("Error: {e}");
        }
    };
    lock_client_return_seed(lightclient, data_dir.into())
}

pub fn save_to_b64() -> String {
    // Return the wallet as a base64 encoded string
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        // we need to use STANDARD because swift is expecting the encoded String with padding
        // I tried with STANDARD_NO_PAD and the decoding return `nil`.
        zingolib::commands::RT.block_on(async move {
            match lightclient.wallet.lock().await.save().await {
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

pub fn execute_command(cmd: String, args_list: String) -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        let args = if args_list.is_empty() {
            vec![]
        } else {
            vec![args_list.as_ref()]
        };
        commands::do_user_command(&cmd, &args, lightclient)
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn get_latest_block_server(server_uri: String) -> String {
    let lightwalletd_uri: http::Uri = server_uri.parse().expect("To be able to represent a Uri.");
    match zingolib::get_latest_block_height(lightwalletd_uri).map_err(|e| format! {"Error: {e}"}) {
        Ok(height) => height.to_string(),
        Err(e) => e,
    }
}

pub fn get_latest_block_wallet() -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        zingolib::commands::RT.block_on(async move {
            object! { "height" => json::JsonValue::from(lightclient.wallet.lock().await.sync_state.wallet_height().map(u32::from).unwrap_or(0))}.pretty(2)
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

pub fn get_transaction_summaries() -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        zingolib::commands::RT
            .block_on(async move { lightclient.transaction_summaries_json_string().await })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn get_value_transfers() -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        zingolib::commands::RT
            .block_on(async move { lightclient.value_transfers_json_string().await })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn set_crypto_default_provider_to_ring() -> String {
    let resp: String;
    {
        if CryptoProvider::get_default().is_none() {
            resp = match default_provider()
                .install_default()
                .map_err(|_| "Error: Failed to install crypto provider".to_string())
            {
                Ok(_) => "true".to_string(),
                Err(e) => e,
            };
        } else {
            resp = "true".to_string();
        };
    }

    resp
}

pub fn poll_sync() -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
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
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        if lightclient.sync_mode() == SyncMode::Paused {
            lightclient.resume_sync().expect("sync should be paused");
            "Resuming sync task...".to_string()
        } else {
            zingolib::commands::RT.block_on(async move {
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
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        match lightclient.pause_sync() {
            Ok(_) => "Pausing sync task...".to_string(),
            Err(e) => format!("Error: {e}"),
        }
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn stop_sync() -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        match lightclient.stop_sync() {
            Ok(_) => "Stopping sync task...".to_string(),
            Err(e) => format!("Error: {e}"),
        }
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn status_sync() -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        zingolib::commands::RT.block_on(async move {
            match pepper_sync::sync_status(&*lightclient.wallet.lock().await).await {
                Ok(status) => json::JsonValue::from(status).pretty(2),
                Err(e) => format!("Error: {e}"),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn run_rescan() -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        zingolib::commands::RT.block_on(async move {
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
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        zingolib::commands::RT.block_on(async move { lightclient.do_info().await })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn get_seed() -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        zingolib::commands::RT.block_on(async move {
            match lightclient.do_seed_phrase().await {
                Ok(m) => serde_json::to_string_pretty(&m).expect("infallible"),
                Err(e) => object! { "error" => e }.pretty(2),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn get_ufvk() -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        zingolib::commands::RT.block_on(async move {
            let wallet = lightclient.wallet.lock().await;
            let ufvk: UnifiedFullViewingKey = match (&wallet.unified_key_store).try_into() {
                Ok(ufvk) => ufvk,
                Err(e) => return e.to_string(),
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
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        if server_uri.is_empty() {
            lightclient.set_server(http::Uri::default());
            "server set (default)".to_string()
        } else {
            match http::Uri::from_str(&server_uri) {
                Ok(uri) => {
                    lightclient.set_server(uri);
                    "server set".to_string()
                }
                Err(_) => "invalid server uri".to_string(),
            }
        }
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn wallet_kind() -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        zingolib::commands::RT.block_on(async move {
            if lightclient.do_seed_phrase().await.is_ok() {
                object! {"kind" => "Loaded from seed phrase",
                        "transparent" => true,
                        "sapling" => true,
                        "orchard" => true,
                }
                .pretty(4)
            } else {
                match &lightclient.wallet.lock().await.unified_key_store {
                    UnifiedKeyStore::Spend(_) => object! {
                        "kind" => "Loaded from unified spending key",
                        "transparent" => true,
                        "sapling" => true,
                        "orchard" => true,
                    }
                    .pretty(4),
                    UnifiedKeyStore::View(ufvk) => object! {
                        "kind" => "Loaded from unified full viewing key",
                        "transparent" => ufvk.transparent().is_some(),
                        "sapling" => ufvk.sapling().is_some(),
                        "orchard" => ufvk.orchard().is_some(),
                    }
                    .pretty(4),
                    UnifiedKeyStore::Empty => object! {
                        "kind" => "No keys found",
                        "transparent" => false,
                        "sapling" => false,
                        "orchard" => false,
                    }
                    .pretty(4),
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
                .to_string(),
                Address::Transparent(_) => object! {
                    "status" => "success",
                    "chain_name" => chain_name_string,
                    "address_kind" => "transparent",
                }
                .to_string(),
                Address::Tex(_) => object! {
                    "status" => "success",
                    "chain_name" => chain_name_string,
                    "address_kind" => "tex",
                }
                .to_string(),
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
                        .to_string()
                    } else {
                        object! {
                            "status" => "success",
                            "chain_name" => chain_name_string,
                            "address_kind" => "unified",
                            "receivers_available" => receivers_available,
                        }
                        .to_string()
                    }
                }
            }
        } else {
            object! {
                "status" => "Invalid address",
                "chain_name" => json::JsonValue::Null,
                "address_kind" => json::JsonValue::Null,
            }
            .to_string()
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
            4,
        )
    }
}

pub fn get_version() -> String {
    zingolib::git_description().to_string()
}

pub fn get_messages(address: String) -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        zingolib::commands::RT.block_on(async move {
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
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        zingolib::commands::RT.block_on(async move {
            serde_json::to_string_pretty(&lightclient.do_balance().await).expect("infallible")
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn get_addresses(receivers: String) -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        let receiver_type = match receivers.as_str() {
            "full" => UAReceivers::All,
            "shielded" => UAReceivers::Shielded,
            "orchard" => UAReceivers::Orchard,
            _ => return "Error: unknown receivers".to_string(),
        };

        zingolib::commands::RT
            .block_on(async move { lightclient.do_addresses(receiver_type).await.pretty(2) })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn get_total_memobytes_to_address() -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        zingolib::commands::RT.block_on(async move {
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
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        zingolib::commands::RT.block_on(async move {
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
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        zingolib::commands::RT.block_on(async move {
            match lightclient.do_total_spends_to_address().await {
                Ok(total_spends) => json::JsonValue::from(total_spends).pretty(2),
                Err(e) => format!("Error: {e}"),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn zec_price() -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        zingolib::commands::RT.block_on(async move {
            let Some(tor_client) = lightclient.tor_client.as_ref() else {
                return "error: no client found. please try restarting.".to_string();
            };
            match lightclient
                .wallet
                .lock()
                .await
                .update_current_price(tor_client)
                .await
            {
                Ok(price) => object! { "current_price" => price },
                Err(e) => {
                    object! { "error" => e.to_string() }
                }
            }
            .pretty(2)
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn zec_price_api_key(key: String) -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        zingolib::commands::RT.block_on(async move {
            lightclient
                .wallet
                .lock()
                .await
                .set_price_api_key(key.to_string());
        });
        "Successfully set API key".to_string()
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn resend_transaction(txid: String) -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        let txid_ok = match txid_from_hex_encoded_str(&txid) {
            Ok(txid) => txid,
            Err(e) => return format!("Error: {e}"),
        };

        zingolib::commands::RT.block_on(async move {
            match lightclient.resend(txid_ok).await {
                Ok(_) => "Successfully resent transaction.".to_string(),
                Err(e) => format!("Error: {e}"),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn remove_transaction(txid: String) -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        let txid_ok = match txid_from_hex_encoded_str(&txid) {
            Ok(txid) => txid,
            Err(e) => return format!("Error: {e}"),
        };

        zingolib::commands::RT.block_on(async move {
            match lightclient
                .wallet
                .lock()
                .await
                .remove_unconfirmed_transaction(txid_ok)
            {
                Ok(_) => "Successfully removed transaction.".to_string(),
                Err(e) => format!("Error: {e}"),
            }
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn get_spendable_balance(address: String, zennies: String) -> String {
    if let Some(lightclient) = &mut *LIGHTCLIENT.lock().unwrap() {
        let address_zcash: ZcashAddress;
        if let Ok(addr) = address_from_str(&address) {
            address_zcash = addr;
        } else {
            return "Error: unknown address format".to_string();
        }
        zingolib::commands::RT.block_on(async move {
            match lightclient
                .get_spendable_shielded_balance(address_zcash, zennies.parse().unwrap_or(false))
                .await
            {
                Ok(bal) => {
                    object! {
                        "balance" => bal.into_u64(),
                    }
                }
                Err(e) => {
                    object! { "error" => e.to_string() }
                }
            }
            .pretty(2)
        })
    } else {
        "Error: Lightclient is not initialized".to_string()
    }
}

pub fn set_option_wallet() -> String {
    "Error unimplemented".to_string()
}

pub fn get_option_wallet() -> String {
    "Error unimplemented".to_string()
}

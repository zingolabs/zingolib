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
use rustls::crypto::ring::default_provider;
use rustls::crypto::CryptoProvider;
use std::sync::Mutex;
use zcash_primitives::consensus::BlockHeight;
use zingolib::config::{construct_lightwalletd_uri, ChainType, RegtestNetwork, ZingoConfig};
use zingolib::{commands, lightclient::LightClient, wallet::LightWallet, wallet::WalletBase};

// We'll use a MUTEX to store a global lightclient instance,
// so we don't have to keep creating it. We need to store it here, in rust
// because we can't return such a complex structure back to JS
lazy_static! {
    static ref LIGHTCLIENT: Mutex<Option<LightClient>> = Mutex::new(None);
}

fn lock_client_return_seed(lightclient: LightClient) -> String {
    LIGHTCLIENT.lock().unwrap().replace(lightclient);

    execute_command("seed".to_string(), "".to_string())
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
    let mut config =
        match zingolib::config::load_clientconfig(lightwalletd_uri.clone(), None, chaintype) {
            Ok(c) => c,
            Err(e) => {
                return Err(format!("Error: Config load: {}", e));
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
    match construct_uri_load_config(server_uri, data_dir, chain_hint) {
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
            return format!("Error: {}", e);
        }
    };
    lock_client_return_seed(lightclient)
}

pub fn init_from_seed(
    server_uri: String,
    seed: String,
    birthday: u64,
    data_dir: String,
    chain_hint: String,
) -> String {
    let (config, _lightwalletd_uri);
    match construct_uri_load_config(server_uri, data_dir, chain_hint) {
        Ok((c, h)) => (config, _lightwalletd_uri) = (c, h),
        Err(s) => return s,
    }

    let wallet = match LightWallet::new(
        config.chain,
        WalletBase::MnemonicPhrase(seed),
        BlockHeight::from_u32(birthday as u32),
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
    lock_client_return_seed(lightclient)
}

pub fn init_from_ufvk(
    server_uri: String,
    ufvk: String,
    birthday: u64,
    data_dir: String,
    chain_hint: String,
) -> String {
    let (config, _lightwalletd_uri);
    match construct_uri_load_config(server_uri, data_dir, chain_hint) {
        Ok((c, h)) => (config, _lightwalletd_uri) = (c, h),
        Err(s) => return s,
    }

    let wallet = match LightWallet::new(
        config.chain,
        WalletBase::Ufvk(ufvk),
        BlockHeight::from_u32(birthday as u32),
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
    lock_client_return_seed(lightclient)
}

pub fn init_from_b64(
    server_uri: String,
    base64_data: String,
    data_dir: String,
    chain_hint: String,
) -> String {
    let (config, _lightwalletd_uri);
    match construct_uri_load_config(server_uri, data_dir, chain_hint) {
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
    lock_client_return_seed(lightclient)
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
                Ok(None) => STANDARD.encode(vec![]),
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

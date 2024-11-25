uniffi::include_scaffolding!("zingo");

#[macro_use]
extern crate lazy_static;
extern crate android_logger;

#[cfg(target_os = "android")]
use android_logger::{Config, FilterBuilder};
#[cfg(target_os = "android")]
use log::Level;
use tokio::runtime::Runtime;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rustls::crypto::ring::default_provider;
use rustls::crypto::CryptoProvider;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use zingolib::config::{construct_lightwalletd_uri, ChainType, RegtestNetwork, ZingoConfig};
use zingolib::{commands, lightclient::LightClient, wallet::WalletBase};

// We'll use a MUTEX to store a global lightclient instance,
// so we don't have to keep creating it. We need to store it here, in rust
// because we can't return such a complex structure back to JS
lazy_static! {
    static ref LIGHTCLIENT: Mutex<RefCell<Option<Arc<LightClient>>>> =
        Mutex::new(RefCell::new(None));
}

fn lock_client_return_seed(
    lightclient: LightClient,
    monitor_mempool: bool,
) -> String {
    let lc = Arc::new(lightclient);

    if monitor_mempool {
        let _ = LightClient::start_mempool_monitor(lc.clone());
    }

    LIGHTCLIENT.lock().unwrap().replace(Some(lc));

    execute_command("seed".to_string(), "".to_string())
}

fn construct_uri_load_config(
    uri: String,
    data_dir: String,
    chain_hint: String,
    monitor_mempool: bool,
) -> Result<(ZingoConfig, http::Uri), String> {
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
        monitor_mempool,
    ) {
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

pub fn init_new(
    server_uri: String,
    data_dir: String,
    chain_hint: String,
    monitor_mempool: bool,
) -> String {
    let (config, lightwalletd_uri);
    match construct_uri_load_config(server_uri, data_dir, chain_hint, monitor_mempool) {
        Ok((c, h)) => (config, lightwalletd_uri) = (c, h),
        Err(s) => return s,
    }
    let latest_block_height = match zingolib::get_latest_block_height(lightwalletd_uri)
        .map_err(|e| format! {"Error: {e}"})
    {
        Ok(height) => height,
        Err(e) => return e,
    };
    let lightclient = match LightClient::new(&config, latest_block_height.saturating_sub(100)) {
        Ok(l) => l,
        Err(e) => {
            return format!("Error: {}", e);
        }
    };
    lock_client_return_seed(lightclient, monitor_mempool)
}

pub fn init_from_seed(
    server_uri: String,
    seed: String,
    birthday: u64,
    data_dir: String,
    chain_hint: String,
    monitor_mempool: bool,
) -> String {
    let (config, _lightwalletd_uri);
    match construct_uri_load_config(server_uri, data_dir, chain_hint, monitor_mempool) {
        Ok((c, h)) => (config, _lightwalletd_uri) = (c, h),
        Err(s) => return s,
    }
    let lightclient = match LightClient::create_from_wallet_base(
        WalletBase::MnemonicPhrase(seed),
        &config,
        birthday,
        false,
    ) {
        Ok(l) => l,
        Err(e) => {
            return format!("Error: {}", e);
        }
    };
    lock_client_return_seed(lightclient, monitor_mempool)
}

pub fn init_from_ufvk(
    server_uri: String,
    ufvk: String,
    birthday: u64,
    data_dir: String,
    chain_hint: String,
    monitor_mempool: bool,
) -> String {
    let (config, _lightwalletd_uri);
    match construct_uri_load_config(server_uri, data_dir, chain_hint, monitor_mempool) {
        Ok((c, h)) => (config, _lightwalletd_uri) = (c, h),
        Err(s) => return s,
    }
    let lightclient = match LightClient::create_from_wallet_base(
        WalletBase::Ufvk(ufvk),
        &config,
        birthday,
        false,
    ) {
        Ok(l) => l,
        Err(e) => {
            return format!("Error: {}", e);
        }
    };
    lock_client_return_seed(lightclient, monitor_mempool)
}

pub fn init_from_b64(
    server_uri: String,
    base64_data: String,
    data_dir: String,
    chain_hint: String,
    monitor_mempool: bool,
) -> String {
    let (config, _lightwalletd_uri);
    match construct_uri_load_config(server_uri, data_dir, chain_hint, monitor_mempool) {
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

    let lightclient =
        match LightClient::read_wallet_from_buffer_runtime(&config, &decoded_bytes[..]) {
            Ok(l) => l,
            Err(e) => {
                return format!("Error: {}", e);
            }
        };
    lock_client_return_seed(lightclient, monitor_mempool)
}

pub fn save_to_b64() -> String {
    // Return the wallet as a base64 encoded string
    let lightclient: Arc<LightClient>;
    {
        let lc = LIGHTCLIENT.lock().unwrap();

        if lc.borrow().is_none() {
            return "Error: Lightclient is not initialized".to_string();
        }

        lightclient = lc.borrow().as_ref().unwrap().clone();
    };

    // we need to use STANDARD because swift is expecting the encoded String with padding
    // I tried with STANDARD_NO_PAD and the decoding return `nil`.
    match lightclient.export_save_buffer_runtime() {
        Ok(buf) => STANDARD.encode(&buf),
        Err(e) => {
            format!("Error: {}", e)
        }
    }
}

pub fn execute_command(cmd: String, args_list: String) -> String {
    let resp: String;
    {
        let lightclient: Arc<LightClient>;
        {
            let lc = LIGHTCLIENT.lock().unwrap();

            if lc.borrow().is_none() {
                return "Error: Lightclient is not initialized".to_string();
            }

            lightclient = lc.borrow().as_ref().unwrap().clone();
        };

        let args = if args_list.is_empty() {
            vec![]
        } else {
            vec![args_list.as_ref()]
        };
        resp = commands::do_user_command(&cmd, &args, lightclient.as_ref()).clone();
    };

    resp
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
    let resp: String;
    {
        let lightclient: Arc<LightClient>;
        {
            let lc = LIGHTCLIENT.lock().unwrap();

            if lc.borrow().is_none() {
                return "Error: Lightclient is not initialized".to_string();
            }

            lightclient = lc.borrow().as_ref().unwrap().clone();
        };

        let rt = Runtime::new().unwrap();
        resp = rt.block_on(async { lightclient.transaction_summaries_json_string().await });
    };

    resp
}

pub fn get_value_transfers() -> String {
    let resp: String;
    {
        let lightclient: Arc<LightClient>;
        {
            let lc = LIGHTCLIENT.lock().unwrap();

            if lc.borrow().is_none() {
                return "Error: Lightclient is not initialized".to_string();
            }

            lightclient = lc.borrow().as_ref().unwrap().clone();
        };

        let rt = Runtime::new().unwrap();
        resp = rt.block_on(async { lightclient.value_transfers_json_string().await });
    };

    resp
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

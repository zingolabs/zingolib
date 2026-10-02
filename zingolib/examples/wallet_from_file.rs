//! # Loading a Wallet from a File Path
//!
//! This example walks through the canonical lifecycle for creating, saving,
//! and reloading a zingolib wallet.
//!
//! ## Design principle
//!
//! zingolib owns all file I/O. Consumers provide a directory path and
//! zingolib opens and writes the wallet file itself.
//!
//! ## The full lifecycle
//!
//! ```text
//! 1. CREATE  : ClientConfig::builder()
//!                .set_wallet_dir(dir)
//!                .set_wallet_config(WalletConfig::MnemonicPhrase { … })
//!                .build()
//!              LightClient::new(config, overwrite)
//!
//! 2. SAVE    : client.save_task().await        ← background writer, runs every second
//!              client.wait_for_save().await    ← wait for first flush
//!              client.shutdown_save_task()     ← clean shutdown
//!
//! 3. RELOAD  : ClientConfig::builder()
//!                .set_wallet_dir(same_dir)
//!                .set_wallet_config(WalletConfig::Read)
//!                .build()
//!              LightClient::new(config, false) ← offline, no network needed
//!
//! 4. CONNECT : client.set_indexer_uri(uri).await   ← go online
//!              client.sync_and_await().await        ← fetch blocks
//! ```
//!
//! ## Running this example
//!
//! ```sh
//! cargo run --example wallet_from_file -p zingolib
//! ```

use std::num::NonZeroU32;
use std::path::PathBuf;

use zingolib::config::{ClientConfig, WalletConfig};
use zingolib::lightclient::LightClient;
use zingolib::wallet::{PerformanceLevel, SyncConfig, TransparentAddressDiscovery, WalletSettings};

/// A 24-word BIP-39 mnemonic used only for this demonstration.
const EXAMPLE_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon art";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ── tracing (optional) ──────────────────────────────────────────────────
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    // ── 1. CHOOSE A WALLET DIRECTORY ────────────────────────────────────────
    //
    // Any writable directory works. Here we use a temporary one so the example
    // is self-contained and leaves no files behind.
    let wallet_dir: PathBuf = {
        let dir = tempfile::tempdir()?;
        dir.keep()
    };
    println!("Wallet directory: {}", wallet_dir.display());

    // ── 2. CREATE WALLET ────────────────────────────────────────────────────
    //
    // Use WalletConfig::MnemonicPhrase to restore from an existing seed, or
    // WalletConfig::NewSeed to generate a fresh wallet with a random mnemonic.
    let create_config = ClientConfig::builder()
        .set_wallet_dir(wallet_dir.clone())
        // No .set_indexer_uri() → offline mode. The wallet can be created and
        // used for offline operations without a network connection.
        .set_wallet_config(WalletConfig::MnemonicPhrase {
            mnemonic_phrase: EXAMPLE_MNEMONIC.to_string(),
            no_of_accounts: NonZeroU32::new(1).unwrap(),
            // Birthday: the Sapling activation height for mainnet is 419_200.
            // Set this to the earliest block at which your wallet may have
            // received funds to minimise the scanning window.
            birthday: 419_200,
            wallet_settings: WalletSettings {
                sync_config: SyncConfig {
                    transparent_address_discovery: TransparentAddressDiscovery::default(),
                    performance_level: PerformanceLevel::High,
                    shutdown_on_completion: true,
                },
                min_confirmations: NonZeroU32::new(3).unwrap(),
            },
        })
        .build()
        .unwrap();

    // `overwrite: true` allows creating a new wallet even if a file already
    // exists at the path (use false in production to avoid accidental overwrites).
    let mut client = LightClient::new(create_config, true).await?;

    println!("Wallet created.");
    println!("  Chain type : {:?}", client.chain_type());
    println!("  Birthday   : {}", client.birthday());
    println!("  Wallet path: {}", client.wallet_path().display());
    println!(
        "  Addresses  : {}",
        client.unified_addresses_json().await.pretty(2)
    );

    // Recovery info contains the seed phrase, birthday, and account count,
    // everything needed to restore the wallet on a new device.
    if let Some(info) = client.recovery_info().await {
        println!("  Seed phrase: {} ... (truncated)", &info.seed_phrase[..20]);
    }

    // ── 3. SAVE TO DISK ─────────────────────────────────────────────────────
    //
    // save_task() spawns a background Tokio task that wakes up every second,
    // checks the `save_required` flag, serialises the wallet if needed, and
    // atomically renames a temp file into place (power-safe).
    //
    // wait_for_save() blocks until the flag clears; shutdown_save_task() then
    // signals the loop to exit and awaits the task.
    client.save_task().await;
    client.wait_for_save().await;
    client.shutdown_save_task().await?;
    println!("\nWallet saved to disk.");

    // Drop the client. The wallet file now lives on disk.
    drop(client);

    // ── 4. RELOAD FROM FILE PATH ────────────────────────────────────────────
    //
    // WalletConfig::Read tells zingolib to open the file at
    // {wallet_dir}/{wallet_name} (default name: "zingo-wallet.dat").
    //
    // All wallet metadata is deserialised: birthday, mnemonic, chain type,
    // addresses, shard trees, and transaction history.
    //
    // Omitting set_indexer_uri() starts the client in offline mode. All
    // local operations (balance, addresses, proposals) are available
    // immediately; call set_indexer_uri() when network access is needed.
    let load_config = ClientConfig::builder()
        .set_wallet_dir(wallet_dir.clone())
        .set_wallet_config(WalletConfig::Read)
        .build()
        .unwrap();

    let reloaded = LightClient::new(load_config, false).await?;

    println!("\nWallet reloaded from file.");
    println!("  Indexer URI: {:?}", reloaded.indexer_uri()); // None, offline
    println!("  Chain type : {:?}", reloaded.chain_type());
    println!("  Birthday   : {}", reloaded.birthday());

    // Offline operations work immediately, before any sync:
    let balance = reloaded.account_balance(zip32::AccountId::ZERO).await?;
    println!("  Balance    : {balance}"); // zero, never synced

    let addrs = reloaded.unified_addresses_json().await;
    println!("  Addresses  : {} (same as before)", addrs.len());

    println!("\nDone.");
    Ok(())
}

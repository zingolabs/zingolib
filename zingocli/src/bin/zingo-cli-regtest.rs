//! Regtest-specific entry point for zingo-cli
//! This binary is only compiled when the regtest feature is enabled
//! It provides a simpler entry point without TLS setup, specifically for regtest mode

#![forbid(unsafe_code)]
#![cfg(feature = "regtest")]

use tracing_subscriber::EnvFilter;

fn main() {
    // Setup logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    // Install TLS provider - might be needed for some connections
    if let Err(e) = rustls::crypto::ring::default_provider().install_default() {
        eprintln!("Error installing crypto provider: {:?}", e);
    }

    println!("Starting zingo-cli in regtest mode...");
    println!("Note: This will launch a local regtest network (zcashd and lightwalletd)");

    // Run CLI in regtest mode
    zingo_cli::run_regtest_cli();
}

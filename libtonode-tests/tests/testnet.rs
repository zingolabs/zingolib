#![forbid(unsafe_code)]
#![cfg(feature = "testnet")]
//! - Dials the public testnet indexer and spends from the shared GloryGoddess testnet wallet.
//! - Waits for real testnet blocks, so each test runs for minutes.

#[path = "testnet/chain_generics.rs"]
mod chain_generics;
#[path = "testnet/chain_metadata.rs"]
mod chain_metadata;
#[path = "testnet/glory_goddess.rs"]
mod glory_goddess;
#[path = "testnet/op_return.rs"]
mod op_return;
#[path = "testnet/reload.rs"]
mod reload;
#[path = "testnet/subtree_roots.rs"]
mod subtree_roots;

const TESTNET_INDEXER: &str = "https://testnet.zec.rocks:443";

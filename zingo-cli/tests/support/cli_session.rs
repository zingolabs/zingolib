//! Constants and helpers shared by the acceptance tests that drive a real
//! `zingo-cli` session over the fixed mainnet window, so the window, the
//! wallet, and the proxy resolution cannot drift apart between tests.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// The number of mainnet blocks below the authoring-day tip the fixed birthday sits.
pub const SYNC_WINDOW: u32 = 20_000;

/// The mainnet chain height on the day the fixed window was authored.
pub const TIP_AT_AUTHORING: u32 = 3_445_000;

/// The fixed wallet birthday, one sync window below the authoring-day tip.
pub const BIRTHDAY: u32 = TIP_AT_AUTHORING - SYNC_WINDOW;

/// The default indexer URI the sessions sync against.
pub const DEFAULT_INDEXER: &str = "https://zec.rocks:443";

/// The cadence at which a harness polls the child session for exit.
pub const CHILD_POLL: Duration = Duration::from_millis(500);

/// A fundless BIP-39 mnemonic, so an interrupted or measured sync scans pure chain data.
pub const MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon abandon \
     abandon abandon abandon abandon abandon abandon abandon art";

/// Returns the nym-proxy path beside the CLI binary, where a mixnet-provisioning startup expects a protocol-matched proxy.
pub fn nym_proxy_beside(cli: &str) -> PathBuf {
    Path::new(cli).with_file_name("nym-proxy")
}

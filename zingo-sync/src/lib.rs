#![warn(missing_docs)]
//! Zingo sync engine prototype
//!
//! Entrypoint: [`crate::sync::sync`]

use zcash_client_backend::data_api::scanning::ScanRange;

pub mod client;
pub mod interface;
pub(crate) mod scanner;
pub mod sync;

/// Encapsulates the current state of sync
pub struct SyncState {
    scan_ranges: Vec<ScanRange>,
}

impl SyncState {
    /// Create new SyncState
    pub fn new() -> Self {
        SyncState {
            scan_ranges: Vec::new(),
        }
    }

    /// TODO: doc comment
    pub fn set_scan_ranges(&mut self) -> &mut Vec<ScanRange> {
        &mut self.scan_ranges
    }
}

impl Default for SyncState {
    fn default() -> Self {
        Self::new()
    }
}

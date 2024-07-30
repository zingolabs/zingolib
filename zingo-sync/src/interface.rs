//! Traits for interfacing a wallet with the sync engine

use crate::SyncState;

/// Temporary dump for all neccessary wallet functionality for PoC
pub trait SyncWallet {
    /// Errors associated with interacting with the wallet data
    type Error: std::fmt::Debug;

    /// Mutable reference to the wallet sync state
    fn set_sync_state(&mut self) -> Result<&mut SyncState, Self::Error>;
}

//! Traits for interfacing a wallet with the sync engine

use std::collections::HashMap;
use std::fmt::Debug;

use zcash_client_backend::keys::UnifiedFullViewingKey;
use zcash_primitives::zip32::AccountId;

use crate::SyncState;

/// Temporary dump for all neccessary wallet functionality for PoC
pub trait SyncWallet {
    /// Errors associated with interacting with the wallet data
    type Error: Debug;

    /// Returns all unified full viewing keys known to this wallet.
    fn get_unified_full_viewing_keys(
        &self,
    ) -> Result<HashMap<AccountId, UnifiedFullViewingKey>, Self::Error>;

    /// Mutable reference to the wallet sync state
    fn set_sync_state(&mut self) -> Result<&mut SyncState, Self::Error>;
}

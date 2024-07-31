//! Traits for interfacing a wallet with the sync engine

use std::collections::HashMap;
use std::fmt::Debug;

use zcash_client_backend::keys::UnifiedFullViewingKey;
use zcash_primitives::transaction::TxId;
use zcash_primitives::zip32::AccountId;

use crate::SyncState;

/// Temporary dump for all neccessary wallet functionality for PoC
pub trait SyncWallet {
    /// Errors associated with interfacing the sync engine with wallet data
    type Error: Debug;

    /// Returns all unified full viewing keys known to this wallet.
    fn get_unified_full_viewing_keys(
        &self,
    ) -> Result<HashMap<AccountId, UnifiedFullViewingKey>, Self::Error>;

    /// Mutable reference to the wallet sync state
    fn set_sync_state(&mut self) -> Result<&mut SyncState, Self::Error>;
}

/// Interface for wallet compact block data
pub trait WalletCompactBlock {
    /// Errors associated with interfacing the sync engine with wallet data
    type Error: Debug;

    /// Get block height
    fn block_height() -> u64;

    /// Get block hash
    fn block_hash() -> Vec<u8>;

    /// Get block hash of previous block
    fn prev_hash() -> Vec<u8>;

    /// Get unix epoch time when the block was mined
    fn time() -> Vec<u8>;

    /// Get list of txids in this block
    fn txids() -> Vec<TxId>;

    /// Get sapling commitment tree size as of the end of this block
    fn sapling_commitment_tree_size() -> u32;

    /// Get orchard commitment tree size as of the end of this block
    fn orchard_commitment_tree_size() -> u32;

    // TODO: read/write
}

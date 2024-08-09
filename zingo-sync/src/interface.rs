//! Traits for interfacing a wallet with the sync engine

use std::collections::HashMap;
use std::fmt::Debug;

use zcash_client_backend::keys::UnifiedFullViewingKey;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::zip32::AccountId;

use crate::primitives::WalletCompactBlock;

/// Temporary dump for all neccessary wallet functionality for PoC
pub trait SyncWallet {
    /// Errors associated with interfacing the sync engine with wallet data
    type Error: Debug;

    /// Returns all unified full viewing keys known to this wallet.
    fn get_unified_full_viewing_keys(
        &self,
    ) -> Result<HashMap<AccountId, UnifiedFullViewingKey>, Self::Error>;
}

/// Trait for interfacing sync engine [`crate::primitives::WalletCompactBlock`] with wallet data
/// Intended to be implemented on - or within - the wallet data struct
pub trait SyncCompactBlocks: SyncWallet {
    /// Get a stored wallet compact block from wallet data by block height
    /// Must return error if block is not found
    fn get_wallet_compact_block(
        &self,
        block_height: BlockHeight,
    ) -> impl std::future::Future<Output = Result<WalletCompactBlock, Self::Error>> + Send;

    /// Store wallet compact blocks in wallet data
    fn store_wallet_compact_blocks(
        &self,
        wallet_compact_blocks: HashMap<BlockHeight, WalletCompactBlock>,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send;
}

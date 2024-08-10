//! Traits for interfacing a wallet with the sync engine

use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;

use zcash_client_backend::keys::UnifiedFullViewingKey;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::zip32::AccountId;

use crate::primitives::{NullifierMap, WalletCompactBlock};

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
    ) -> Result<WalletCompactBlock, Self::Error>;

    /// Append wallet compact blocks to wallet data
    fn append_wallet_compact_blocks(
        &mut self,
        wallet_compact_blocks: BTreeMap<BlockHeight, WalletCompactBlock>,
    ) -> Result<(), Self::Error>;
}

/// Trait for interfacing nullifiers with wallet data
/// Intended to be implemented on - or within - the wallet data struct
pub trait SyncNullifiers: SyncWallet {
    /// Append nullifiers to wallet data
    fn append_nullifiers(&mut self, nullifier_map: NullifierMap) -> Result<(), Self::Error>;
}

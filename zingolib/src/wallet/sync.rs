//! Trait implementations for interfacing [`crate::wallet::LightWallet`] with [`pepper_sync`] sync engine.

use std::collections::{BTreeMap, HashMap};

use pepper_sync::{
    keys::transparent::TransparentAddressId,
    wallet::traits::{
        SyncBlocks, SyncNullifiers, SyncOutPoints, SyncShardTrees, SyncTransactions, SyncWallet,
    },
    wallet::{Locator, NullifierMap, OutputId, ShardTrees, SyncState, WalletBlock},
};
use zcash_keys::keys::UnifiedFullViewingKey;
use zcash_primitives::consensus::BlockHeight;
use zip32::AccountId;

use super::{LightWallet, error::WalletError};

impl SyncWallet for LightWallet {
    type Error = WalletError;

    fn get_birthday(&self) -> Result<BlockHeight, Self::Error> {
        Ok(self.birthday)
    }

    fn get_sync_state(&self) -> Result<&SyncState, Self::Error> {
        Ok(&self.sync_state)
    }

    fn get_sync_state_mut(&mut self) -> Result<&mut SyncState, Self::Error> {
        Ok(&mut self.sync_state)
    }

    fn get_unified_full_viewing_keys(
        &self,
    ) -> Result<HashMap<AccountId, UnifiedFullViewingKey>, Self::Error> {
        let account_id = AccountId::try_from(0).expect("valid hard-coded u32");
        let ufvk = UnifiedFullViewingKey::try_from(&self.unified_key_store)?;
        let mut ufvk_map = HashMap::new();
        ufvk_map.insert(account_id, ufvk);

        Ok(ufvk_map)
    }

    fn get_transparent_addresses(
        &self,
    ) -> Result<&BTreeMap<TransparentAddressId, String>, Self::Error> {
        Ok(&self.transparent_addresses)
    }

    fn get_transparent_addresses_mut(
        &mut self,
    ) -> Result<&mut BTreeMap<TransparentAddressId, String>, Self::Error> {
        Ok(&mut self.transparent_addresses)
    }

    fn set_save_flag(&mut self) -> Result<(), Self::Error> {
        self.save_required = true;
        Ok(())
    }
}

impl SyncBlocks for LightWallet {
    fn get_wallet_block(&self, block_height: BlockHeight) -> Result<WalletBlock, Self::Error> {
        self.wallet_blocks
            .get(&block_height)
            .cloned()
            .ok_or(WalletError::BlockNotFound(block_height))
    }

    fn get_wallet_blocks_mut(
        &mut self,
    ) -> Result<&mut BTreeMap<BlockHeight, WalletBlock>, Self::Error> {
        Ok(&mut self.wallet_blocks)
    }
}

impl SyncTransactions for LightWallet {
    fn get_wallet_transactions(
        &self,
    ) -> Result<
        &HashMap<zcash_primitives::transaction::TxId, pepper_sync::wallet::WalletTransaction>,
        Self::Error,
    > {
        Ok(&self.wallet_transactions)
    }

    fn get_wallet_transactions_mut(
        &mut self,
    ) -> Result<
        &mut HashMap<zcash_primitives::transaction::TxId, pepper_sync::wallet::WalletTransaction>,
        Self::Error,
    > {
        Ok(&mut self.wallet_transactions)
    }
}

impl SyncNullifiers for LightWallet {
    fn get_nullifiers(&self) -> Result<&NullifierMap, Self::Error> {
        Ok(&self.nullifier_map)
    }

    fn get_nullifiers_mut(&mut self) -> Result<&mut NullifierMap, Self::Error> {
        Ok(&mut self.nullifier_map)
    }
}

impl SyncOutPoints for LightWallet {
    fn get_outpoints(&self) -> Result<&BTreeMap<OutputId, Locator>, Self::Error> {
        Ok(&self.outpoint_map)
    }

    fn get_outpoints_mut(&mut self) -> Result<&mut BTreeMap<OutputId, Locator>, Self::Error> {
        Ok(&mut self.outpoint_map)
    }
}

impl SyncShardTrees for LightWallet {
    fn get_shard_trees(&self) -> Result<&ShardTrees, Self::Error> {
        Ok(&self.shard_trees)
    }

    fn get_shard_trees_mut(&mut self) -> Result<&mut ShardTrees, Self::Error> {
        Ok(&mut self.shard_trees)
    }
}
